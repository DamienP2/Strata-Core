//! Les serveurs de stockage (Ce serveur › Stockage) : où le Core range des fichiers — un partage
//! d'un NAS (SMB 2/3, par `smbclient`), SFTP, FTP ou FTPS, WebDAV (par `curl`), ou un dossier du
//! serveur (un NAS monté là). Repris de Strata LiveCut (ses serveurs de fichiers).
//!
//! Les mots de passe sont masqués pour tous, et jamais sur une ligne de commande : dans un fichier
//! au seul compte du service, le temps d'un envoi ; un mot de passe gardé ne part que vers le
//! serveur et le compte pour lesquels il a été donné.
use axum::{
    extract::{Path as UrlPath, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::process::Command;

use crate::{
    api::{bad, require, ApiResult},
    auth::CurrentUser,
    state::Shared,
};

pub const KINDS: [&str; 5] = ["smb", "sftp", "ftp", "webdav", "dir"];
const MASKED: &str = "••••••••";

/// Un serveur de stockage.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Target {
    pub id: String,
    pub name: String,
    /// smb, sftp, ftp, webdav ou dir (un dossier du serveur).
    pub kind: String,
    pub host: String,
    /// 0 : celui du protocole.
    pub port: u16,
    /// SMB : le partage.
    pub share: String,
    /// Le dossier là-bas (dans le partage, sur le serveur) ; dir : le dossier du serveur.
    pub path: String,
    pub user: String,
    pub password: String,
    /// FTP : explicit (FTPS), implicit (FTPS) ou none ; WebDAV : tls (https) ou none.
    pub tls: String,
}

impl Default for Target {
    fn default() -> Self {
        Target {
            id: String::new(),
            name: String::new(),
            kind: "smb".into(),
            host: String::new(),
            port: 0,
            share: String::new(),
            path: String::new(),
            user: String::new(),
            password: String::new(),
            tls: "explicit".into(),
        }
    }
}

impl Target {
    pub fn normalize(&mut self) {
        for v in [&mut self.name, &mut self.host, &mut self.share, &mut self.path, &mut self.user] {
            *v = v.trim().to_string();
        }
        self.share = self.share.trim_matches(['/', '\\']).to_string();
        let tls_ok = match self.kind.as_str() {
            "ftp" => matches!(self.tls.as_str(), "explicit" | "implicit" | "none"),
            "webdav" => matches!(self.tls.as_str(), "tls" | "none"),
            _ => true,
        };
        if !tls_ok {
            self.tls = if self.kind == "ftp" { "explicit".into() } else { "tls".into() };
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("the name cannot be empty".into());
        }
        if !KINDS.contains(&self.kind.as_str()) {
            return Err("unknown kind of storage".into());
        }
        let parts = |p: &str| p.split(['/', '\\']).any(|s| s == "..");
        if parts(&self.path) || parts(&self.share) {
            return Err("no « .. » in the folders".into());
        }
        if self.kind == "dir" {
            if !self.path.starts_with('/') {
                return Err("the server's folder is a full path (/mnt/…)".into());
            }
        } else {
            if self.host.is_empty() || self.host.starts_with('-') || self.host.chars().any(|c| c.is_whitespace() || "/\\@?#".contains(c)) {
                return Err("the server's name or address is not valid".into());
            }
            if self.kind == "smb" && self.share.is_empty() {
                return Err("give the share".into());
            }
        }
        if [&self.name, &self.user, &self.path, &self.share, &self.password].iter().any(|v| v.chars().any(char::is_control)) {
            return Err("no control characters".into());
        }
        Ok(())
    }

    fn default_port(&self) -> u16 {
        match (self.kind.as_str(), self.tls.as_str()) {
            ("smb", _) => 445,
            ("sftp", _) => 22,
            ("ftp", "implicit") => 990,
            ("ftp", _) => 21,
            ("webdav", "none") => 80,
            ("webdav", _) => 443,
            _ => 0,
        }
    }

    fn port(&self) -> u16 {
        if self.port > 0 {
            self.port
        } else {
            self.default_port()
        }
    }

    /// Où c'est, tel qu'on le montre (sans mot de passe).
    pub fn address(&self) -> String {
        let port = if self.port > 0 && self.port != self.default_port() { format!(":{}", self.port) } else { String::new() };
        let path = self.path.trim_matches('/');
        let path = if path.is_empty() { String::new() } else { format!("/{path}") };
        match self.kind.as_str() {
            "dir" => self.path.clone(),
            "smb" => format!("smb://{}{port}/{}{path}", self.host, self.share),
            "ftp" => format!("{}://{}{port}{path}", if self.tls == "none" { "ftp" } else { "ftps" }, self.host),
            "webdav" => format!("{}://{}{port}{path}", if self.tls == "none" { "http" } else { "https" }, self.host),
            k => format!("{k}://{}{port}{path}", self.host),
        }
    }

    fn public(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if !self.password.is_empty() {
            v["password"] = json!(MASKED);
        }
        v["address"] = json!(self.address());
        v
    }

    fn same_place(&self, o: &Target) -> bool {
        (self.kind.as_str(), self.host.to_lowercase(), self.port(), self.user.as_str()) == (o.kind.as_str(), o.host.to_lowercase(), o.port(), o.user.as_str())
    }
}

fn path_of(data_dir: &Path) -> PathBuf {
    data_dir.join("storage.json")
}

pub fn load(data_dir: &Path) -> Vec<Target> {
    strata_core::files::read_json(&path_of(data_dir)).unwrap_or_default()
}

fn save(data_dir: &Path, all: &[Target]) -> Result<(), String> {
    strata_core::files::write_json(&path_of(data_dir), &all, true).map_err(|e| format!("cannot save the storage servers: {e}"))
}

pub fn find(data_dir: &Path, id: &str) -> Option<Target> {
    load(data_dir).into_iter().find(|t| t.id == id)
}

/// Un nom sûr pour un dossier ou un fichier.
pub fn safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '-' } else { c }).collect();
    t.trim().trim_matches('.').trim().to_string()
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Ce qu'un outil dit de son échec, en clair.
fn explain(kind: &str, code: Option<i32>, said: &str) -> String {
    let low = said.to_lowercase();
    let by_code = match (kind, code) {
        (_, Some(6)) => Some("the server's name is unknown"),
        (_, Some(7)) => Some("no answer from the server (address, port or firewall)"),
        (_, Some(28)) => Some("the server stopped answering"),
        (_, Some(67)) => Some("the server refuses the user or the password"),
        (_, Some(9)) => Some("access to the folder refused"),
        (_, Some(25)) => Some("the server refuses the file (rights, or no space)"),
        ("sftp", Some(60)) => Some("the server's key changed: is it the right server? (Test learns it again)"),
        (_, Some(35)) | (_, Some(60)) => Some("the server's certificate is not valid"),
        (_, Some(51)) | (_, Some(83)) => Some("the server's key changed: is it the right server?"),
        _ => None,
    };
    if let Some(r) = by_code {
        return r.into();
    }
    if low.contains("logon_failure") || low.contains("access denied") || low.contains("login denied") || low.contains("401") {
        return "the server refuses the user or the password".into();
    }
    if low.contains("bad_network_name") {
        return "this share does not exist on the server".into();
    }
    if low.contains("access_denied") || low.contains("403") || low.contains("permission denied") {
        return "access to the folder refused".into();
    }
    if low.contains("disk_full") || low.contains("507") || low.contains("no space") {
        return "the server has no space left".into();
    }
    if low.contains("connection refused")
        || low.contains("connection_refused")
        || low.contains("host_unreachable")
        || low.contains("timed out")
        || low.contains("io_timeout")
    {
        return "no answer from the server (address, port or firewall)".into();
    }
    let line = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.is_empty() {
        "the transfer failed".into()
    } else {
        line.chars().take(160).collect()
    }
}

/// Un fichier ôté quand il part.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Les échanges avec un serveur de stockage.
pub struct Transfer<'a> {
    pub target: &'a Target,
    /// Où vont les fichiers de mot de passe et de test (au seul compte du service).
    pub temp: PathBuf,
}

impl Transfer<'_> {
    pub fn new<'a>(target: &'a Target, data_dir: &Path) -> Transfer<'a> {
        Transfer { target, temp: data_dir.join("storage-tmp") }
    }

    fn url(&self, dir: &str, name: &str) -> String {
        let t = self.target;
        let scheme = match (t.kind.as_str(), t.tls.as_str()) {
            ("ftp", "implicit") => "ftps",
            ("ftp", _) => "ftp",
            ("webdav", "none") => "http",
            ("webdav", _) => "https",
            (k, _) => k,
        };
        let host = if t.host.contains(':') { format!("[{}]", t.host) } else { t.host.clone() };
        let enc = |p: &str| p.split('/').filter(|x| !x.is_empty()).map(urlencode).collect::<Vec<_>>().join("/");
        let mut path = enc(dir);
        if t.kind == "sftp" && !t.path.starts_with('/') {
            path = format!("~/{path}");
        }
        let mut url = format!("{scheme}://{host}:{}/{path}", t.port());
        if !url.ends_with('/') {
            url.push('/');
        }
        url.push_str(&urlencode(name));
        url
    }

    async fn secret_file(&self, text: &str) -> Result<TempFile, String> {
        tokio::fs::create_dir_all(&self.temp).await.map_err(|e| e.to_string())?;
        let path = self.temp.join(format!(".{}", strata_core::new_id()));
        strata_core::files::write_atomic(&path, text.as_bytes(), true).map_err(|e| e.to_string())?;
        Ok(TempFile(path))
    }

    /// curl, son utilisateur et son mot de passe dans un fichier : (code de sortie, ce qu'il a dit).
    async fn curl(&self, args: &[String], max_secs: u64) -> Result<(Option<i32>, String), String> {
        let t = self.target;
        let quote = |v: &str| v.replace('\\', "\\\\").replace('"', "\\\"");
        let conf = if t.user.is_empty() {
            if t.kind == "ftp" {
                "user = \"anonymous:\"\n".to_string()
            } else {
                String::new()
            }
        } else {
            format!("user = \"{}:{}\"\n", quote(&t.user), quote(&t.password))
        };
        let file = self.secret_file(&conf).await?;
        let home = self.temp.join("home");
        let _ = tokio::fs::create_dir_all(home.join(".ssh")).await;
        if t.kind == "sftp" {
            self.know_host(false).await?;
        }
        let mut cmd = Command::new("curl");
        cmd.env("HOME", &home)
            .arg("--config")
            .arg(&file.0)
            .args(["--connect-timeout", "15", "--max-time", &max_secs.to_string(), "--fail", "-sS"])
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if t.kind == "ftp" && t.tls == "explicit" {
            cmd.arg("--ssl-reqd");
        }
        if t.kind == "ftp" && t.tls != "none" {
            // Les serveurs FTPS d'un réseau local ont le plus souvent leur propre certificat.
            cmd.arg("--insecure");
        }
        let out = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "curl is not installed on the server".to_string()
            } else {
                format!("cannot start curl: {e}")
            }
        })?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok((out.status.code(), text))
    }

    /// SFTP : la clé du serveur, apprise la première fois (puis vérifiée par curl).
    async fn know_host(&self, again: bool) -> Result<(), String> {
        let t = self.target;
        let file = self.temp.join("home").join(".ssh").join("known_hosts");
        let key = if t.port() == 22 { t.host.to_lowercase() } else { format!("[{}]:{}", t.host.to_lowercase(), t.port()) };
        let known = tokio::fs::read_to_string(&file).await.unwrap_or_default();
        let mine = |l: &str| l.split_whitespace().next().is_some_and(|h| h.eq_ignore_ascii_case(&key));
        if !again && known.lines().any(mine) {
            return Ok(());
        }
        let out = Command::new("ssh-keyscan")
            .args(["-T", "10", "-p", &t.port().to_string(), "--", &t.host])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| format!("cannot start ssh-keyscan: {e}"))?;
        let found: Vec<String> =
            String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).map(String::from).collect();
        if found.is_empty() {
            return Err("no answer from the server (address, port or firewall)".into());
        }
        let mut lines: Vec<String> = known.lines().filter(|l| !mine(l)).map(String::from).collect();
        lines.extend(found);
        strata_core::files::write_atomic(&file, (lines.join("\n") + "\n").as_bytes(), true).map_err(|e| e.to_string())
    }

    /// smbclient sur le partage, utilisateur et mot de passe dans un fichier : (réussi, ce qu'il a dit).
    async fn smb(&self, dir: &str, command: &str) -> Result<(bool, String), String> {
        let t = self.target;
        let auth = self.secret_file(&format!("username = {}\npassword = {}\n", t.user, t.password)).await?;
        let mut cmd = Command::new("smbclient");
        cmd.arg(format!("//{}/{}", t.host, t.share)).arg("-A").arg(&auth.0).args(["-p", &t.port().to_string(), "-m", "SMB3", "-t", "30"]);
        if !dir.is_empty() {
            cmd.arg("-D").arg(dir.replace('/', "\\"));
        }
        cmd.arg("-c").arg(command).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let out = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "smbclient is not installed on the server".to_string()
            } else {
                format!("cannot start smbclient: {e}")
            }
        })?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let failed = text.contains("NT_STATUS_") && !text.contains("NT_STATUS_OBJECT_NAME_COLLISION");
        Ok((out.status.success() && !failed, text))
    }

    /// Ce qu'il y a dans un dossier là-bas : (nom, est un dossier). None s'il n'y est pas.
    pub async fn entries(&self, dir: &str) -> Result<Option<Vec<(String, bool)>>, String> {
        let t = self.target;
        let mut out: Vec<(String, bool)> = match t.kind.as_str() {
            "dir" => {
                let mut rd = match tokio::fs::read_dir(Path::new(&t.path).join(dir)).await {
                    Ok(rd) => rd,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(format!("cannot read this folder: {e}")),
                };
                let mut v = Vec::new();
                while let Ok(Some(e)) = rd.next_entry().await {
                    let is_dir = e.file_type().await.is_ok_and(|f| f.is_dir());
                    v.push((e.file_name().to_string_lossy().into_owned(), is_dir));
                }
                v
            }
            "smb" => {
                let (ok, said) = self.smb(dir, "ls").await?;
                if said.contains("NT_STATUS_OBJECT_NAME_NOT_FOUND")
                    || said.contains("NT_STATUS_OBJECT_PATH_NOT_FOUND")
                    || said.contains("NT_STATUS_NOT_A_DIRECTORY")
                {
                    return Ok(None);
                }
                if !ok {
                    return Err(explain("smb", None, &said));
                }
                said.lines()
                    .filter_map(|l| {
                        let l = l.trim_end();
                        if !l.chars().any(|c| c.is_ascii_digit()) {
                            return None;
                        }
                        let parts: Vec<&str> = l.split("  ").map(str::trim).filter(|x| !x.is_empty()).collect();
                        (parts.len() >= 3).then(|| (parts[0].to_string(), parts[1].contains('D')))
                    })
                    .collect()
            }
            "webdav" => {
                let url = self.url(dir, "");
                let (code, said) = self.curl(&["-X".into(), "PROPFIND".into(), "-H".into(), "Depth: 1".into(), url.clone()], 30).await?;
                if code == Some(22) && said.contains("404") {
                    return Ok(None);
                }
                if code != Some(0) {
                    return Err(explain("webdav", code, &said));
                }
                let own = url.split_once("://").and_then(|(_, r)| r.split_once('/')).map(|(_, p)| format!("/{p}")).unwrap_or_default();
                said.split("response>")
                    .filter_map(|r| {
                        let a = r.find("href>")? + 5;
                        let b = r[a..].find('<')? + a;
                        let href = r[a..b].trim().trim_end_matches('/');
                        let path =
                            href.split_once("://").and_then(|(_, r)| r.split_once('/')).map(|(_, p)| format!("/{p}")).unwrap_or_else(|| href.to_string());
                        (path.trim_end_matches('/') != own.trim_end_matches('/'))
                            .then(|| (decode(path.rsplit('/').next().unwrap_or("")), r.contains("collection")))
                    })
                    .filter(|(n, _)| !n.is_empty())
                    .collect()
            }
            kind => {
                let (code, said) = self.curl(&[self.url(dir, "")], 30).await?;
                if matches!(code, Some(9) | Some(78)) {
                    return Ok(None);
                }
                if code != Some(0) {
                    return Err(explain(kind, code, &said));
                }
                said.lines()
                    .filter(|l| l.starts_with('d') || l.starts_with('-'))
                    .filter_map(|l| l.split_whitespace().nth(8).map(|_| (l.split_whitespace().skip(8).collect::<Vec<_>>().join(" "), l.starts_with('d'))))
                    .collect()
            }
        };
        out.retain(|(n, _)| n != "." && n != ".." && !n.starts_with('.'));
        out.sort_by_key(|(n, _)| n.to_lowercase());
        out.dedup();
        Ok(Some(out))
    }

    /// Envoie un fichier (les dossiers faits en chemin), sous un nom provisoire puis le bon.
    pub async fn put(&self, file: &Path, dir: &str, name: &str) -> Result<(), String> {
        let t = self.target;
        match t.kind.as_str() {
            "dir" => {
                let to = Path::new(&t.path).join(dir);
                tokio::fs::create_dir_all(&to).await.map_err(|e| format!("cannot make the folder there: {e}"))?;
                let part = to.join(format!(".{name}.part"));
                tokio::fs::copy(file, &part).await.map_err(|e| {
                    if e.raw_os_error() == Some(28) {
                        "the server has no space left".to_string()
                    } else {
                        format!("cannot write there: {e}")
                    }
                })?;
                tokio::fs::rename(&part, to.join(name)).await.map_err(|e| e.to_string())
            }
            "smb" => {
                let mut made = String::new();
                for part in dir.split('/').filter(|p| !p.is_empty()) {
                    made = if made.is_empty() { part.to_string() } else { format!("{made}\\{part}") };
                    let (ok, said) = self.smb("", &format!("mkdir \"{made}\"")).await?;
                    if !ok && !said.contains("COLLISION") {
                        return Err(explain("smb", None, &said));
                    }
                }
                let local = file.to_string_lossy().replace('"', "");
                let (ok, said) = self.smb(dir, &format!("put \"{local}\" \"{name}\"")).await?;
                if ok {
                    Ok(())
                } else {
                    Err(explain("smb", None, &said))
                }
            }
            kind => {
                if kind == "webdav" {
                    self.mkcol(dir).await?;
                }
                let mut args = vec!["-T".to_string(), file.to_string_lossy().into_owned(), self.url(dir, name)];
                if kind != "webdav" {
                    args.insert(0, "--ftp-create-dirs".into());
                }
                let (code, said) = self.curl(&args, 3600).await?;
                if code == Some(0) {
                    Ok(())
                } else {
                    Err(explain(kind, code, &said))
                }
            }
        }
    }

    /// Efface un fichier là-bas.
    pub async fn remove(&self, dir: &str, name: &str) -> Result<(), String> {
        let t = self.target;
        match t.kind.as_str() {
            "dir" => tokio::fs::remove_file(Path::new(&t.path).join(dir).join(name)).await.map_err(|e| e.to_string()),
            "smb" => {
                let (ok, said) = self.smb(dir, &format!("del \"{name}\"")).await?;
                if ok {
                    Ok(())
                } else {
                    Err(explain("smb", None, &said))
                }
            }
            "webdav" => {
                let (code, said) = self.curl(&["-X".into(), "DELETE".into(), self.url(dir, name)], 30).await?;
                if code == Some(0) {
                    Ok(())
                } else {
                    Err(explain("webdav", code, &said))
                }
            }
            "ftp" => {
                let (code, said) = self
                    .curl(&["-Q".into(), format!("DELE {}{name}", if dir.is_empty() { String::new() } else { format!("{dir}/") }), self.url("", "")], 30)
                    .await?;
                if code == Some(0) {
                    Ok(())
                } else {
                    Err(explain("ftp", code, &said))
                }
            }
            _ => {
                let root = if t.path.starts_with('/') { "/" } else { "" };
                let (code, said) = self
                    .curl(
                        &["-Q".into(), format!("rm \"{root}{}{name}\"", if dir.is_empty() { String::new() } else { format!("{dir}/") }), self.url("", "")],
                        30,
                    )
                    .await?;
                if code == Some(0) {
                    Ok(())
                } else {
                    Err(explain("sftp", code, &said))
                }
            }
        }
    }

    async fn mkcol(&self, dir: &str) -> Result<(), String> {
        let mut made = String::new();
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            made = if made.is_empty() { part.to_string() } else { format!("{made}/{part}") };
            let (code, said) = self.curl(&["-X".into(), "MKCOL".into(), self.url(&made, "")], 30).await?;
            if code != Some(0) && !(code == Some(22) && (said.contains("405") || said.contains("301"))) {
                return Err(explain("webdav", code, &said));
            }
        }
        Ok(())
    }

    /// Le dossier de base là-bas (celui du serveur), tel que les envois l'utilisent.
    pub fn base(&self) -> String {
        let t = self.target;
        if t.kind == "dir" {
            String::new()
        } else {
            t.path.trim_matches(['/', '\\']).to_string()
        }
    }

    /// Écrit un petit fichier dans le dossier du serveur, puis l'efface : Ok(ms).
    pub async fn test(&self) -> Result<u64, String> {
        let t = self.target;
        let started = Instant::now();
        let name = format!("strata-test-{}.txt", strata_core::new_id());
        tokio::fs::create_dir_all(&self.temp).await.map_err(|e| e.to_string())?;
        let local = self.temp.join(&name);
        tokio::fs::write(&local, b"Strata Core: test\n").await.map_err(|e| e.to_string())?;
        let _gone = TempFile(local.clone());
        if t.kind == "sftp" {
            self.know_host(true).await?;
        }
        if t.kind == "dir" && !Path::new(&t.path).is_dir() {
            return Err("this folder does not exist on the server".into());
        }
        let base = self.base();
        self.put(&local, &base, &name).await?;
        self.remove(&base, &name).await?;
        Ok(started.elapsed().as_millis() as u64)
    }
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------- API (administrateurs) ----------

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/storage", get(list).post(create))
        .route("/api/storage/test", post(test))
        .route("/api/storage/browse", post(browse))
        .route("/api/storage/{id}", put(update).delete(remove))
}

async fn list(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    Ok(Json(Value::Array(load(&s.data_dir).iter().map(Target::public).collect())))
}

/// Le mot de passe gardé quand la page l'envoie masqué, pour le même serveur et compte seulement.
fn keep_password(t: &mut Target, old: Option<&Target>) -> Result<(), String> {
    if t.password == MASKED {
        match old {
            Some(o) if o.same_place(t) => t.password = o.password.clone(),
            Some(o) if !o.password.is_empty() => return Err("give the password again for this server".into()),
            _ => t.password.clear(),
        }
    }
    Ok(())
}

static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

async fn create(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut t): Json<Target>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    t.normalize();
    keep_password(&mut t, None).map_err(bad)?;
    t.validate().map_err(bad)?;
    t.id = strata_core::new_id();
    let _w = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(&s.data_dir);
    all.push(t.clone());
    save(&s.data_dir, &all).map_err(bad)?;
    tracing::info!("{} adds the storage server {} ({})", me.username, t.name, t.address());
    Ok(Json(t.public()))
}

async fn update(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>, Json(mut t): Json<Target>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let _w = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(&s.data_dir);
    let i = all.iter().position(|x| x.id == id).ok_or((StatusCode::NOT_FOUND, "no such storage server".to_string()))?;
    t.normalize();
    keep_password(&mut t, Some(&all[i])).map_err(bad)?;
    t.validate().map_err(bad)?;
    t.id = id;
    all[i] = t.clone();
    save(&s.data_dir, &all).map_err(bad)?;
    Ok(Json(t.public()))
}

async fn remove(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<StatusCode> {
    require(me.admin)?;
    let _w = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(&s.data_dir);
    let before = all.len();
    all.retain(|t| t.id != id);
    if all.len() == before {
        return Err((StatusCode::NOT_FOUND, "no such storage server".into()));
    }
    save(&s.data_dir, &all).map_err(bad)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Essaie un serveur tel qu'il est dans le formulaire (son mot de passe gardé s'il est masqué).
async fn test(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut t): Json<Target>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    static TESTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _one = TESTS.try_acquire().map_err(|_| bad("other tests are under way: again in a moment"))?;
    t.normalize();
    let old = find(&s.data_dir, &t.id);
    keep_password(&mut t, old.as_ref()).map_err(bad)?;
    t.validate().map_err(bad)?;
    let tr = Transfer::new(&t, &s.data_dir);
    let ms = tokio::time::timeout(Duration::from_secs(60), tr.test())
        .await
        .map_err(|_| bad("no answer from the server (address, port or firewall)"))?
        .map_err(bad)?;
    Ok(Json(json!({ "ok": true, "ms": ms })))
}

#[derive(Deserialize)]
struct Browse {
    target: Target,
    #[serde(default)]
    path: Option<String>,
}

/// Les dossiers d'un dossier là-bas, pour en choisir un.
async fn browse(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(b): Json<Browse>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let mut t = b.target;
    t.normalize();
    let old = find(&s.data_dir, &t.id);
    keep_password(&mut t, old.as_ref()).map_err(bad)?;
    let path = b.path.unwrap_or_else(|| t.path.clone());
    if path.split(['/', '\\']).any(|p| p == "..") {
        return Err(bad("no « .. » in the folders"));
    }
    t.path = path.clone();
    t.validate().map_err(bad)?;
    // Le dossier lui-même, depuis la racine du serveur (ou dans le dossier, pour « dir »).
    let dir = if t.kind == "dir" { String::new() } else { path.trim_matches('/').to_string() };
    let tr = Transfer::new(&t, &s.data_dir);
    let r = tokio::time::timeout(Duration::from_secs(30), tr.entries(&dir)).await.map_err(|_| bad("no answer from the server (address, port or firewall)"))?;
    match r.map_err(bad)? {
        Some(e) => Ok(Json(json!({ "exists": true, "path": path, "folders": e.into_iter().filter(|(_, d)| *d).map(|(n, _)| n).collect::<Vec<_>>() }))),
        None => Ok(Json(json!({ "exists": false, "path": path, "folders": [] }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_and_addresses() {
        let mut t = Target {
            name: "NAS".into(),
            kind: "smb".into(),
            host: "nas.local".into(),
            share: "/Régie/".into(),
            path: "sauvegardes".into(),
            ..Default::default()
        };
        t.normalize();
        assert!(t.validate().is_ok());
        assert_eq!(t.address(), "smb://nas.local/Régie/sauvegardes");
        for bad in [Target { host: "-oProxy".into(), ..t.clone() }, Target { path: "../etc".into(), ..t.clone() }, Target { share: String::new(), ..t.clone() }]
        {
            assert!(bad.validate().is_err());
        }
        let d = Target { name: "Disque".into(), kind: "dir".into(), path: "mnt/nas".into(), ..Default::default() };
        assert!(d.validate().is_err(), "a full path");
        let mut f = Target { name: "FTP".into(), kind: "ftp".into(), host: "f".into(), tls: "weird".into(), ..Default::default() };
        f.normalize();
        assert_eq!((f.tls.as_str(), f.port()), ("explicit", 21));
    }

    #[test]
    fn a_folder_of_the_server_round_trip() {
        let root = std::env::temp_dir().join(format!("strata-storage-{}", strata_core::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let t = Target { name: "Dossier".into(), kind: "dir".into(), path: root.display().to_string(), ..Default::default() };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let tr = Transfer::new(&t, &root);
            assert!(tr.test().await.is_ok());
            let f = root.join("x.tar.gz");
            std::fs::write(&f, b"x").unwrap();
            tr.put(&f, "Régie A", "a.tar.gz").await.unwrap();
            let e = tr.entries("Régie A").await.unwrap().unwrap();
            assert_eq!(e, vec![("a.tar.gz".to_string(), false)]);
            tr.remove("Régie A", "a.tar.gz").await.unwrap();
            assert!(tr.entries("Régie A").await.unwrap().unwrap().is_empty());
            assert!(tr.entries("absent").await.unwrap().is_none());
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
