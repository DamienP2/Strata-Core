//! Ce que fait l'installateur, étape par étape : installer, mettre à jour, réparer, désinstaller.
//! Avec un dossier racine (`--root`), les fichiers vont sous ce dossier et les commandes du système
//! sont seulement écrites au journal (essai à blanc) ; le Core, lui, est vraiment lancé.
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::{
    paths::{self, under},
    release,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Install,
    Update,
    Repair,
    Uninstall { purge: bool },
}

/// Ce qui est demandé (par l'écran ou par les options).
#[derive(Clone, Debug)]
pub struct Plan {
    pub action: Action,
    /// Le nom d'usage (« Régie A »), le nom DNS, le fuseau.
    pub name: String,
    pub dns_name: String,
    pub timezone: String,
    /// Les applis : dépôts GitHub (`owner/repo`) ou archives ; activées une fois installées.
    pub apps: Vec<String>,
    /// Le pilote Blackmagic : son archive (ou son paquet .deb).
    pub desktop_video: Option<PathBuf>,
    /// La bibliothèque NDI, sortie du SDK et sa licence acceptée.
    pub ndi: Option<PathBuf>,
    /// Le pilote NVIDIA (pour NVENC/NVDEC), et les paquets VA-API (Intel, AMD).
    pub nvidia: bool,
    pub vaapi: Vec<String>,
    /// Ouvrir les ports du cockpit dans le pare-feu (ufw actif).
    pub firewall: bool,
    /// Le compte administrateur : nom, mot de passe.
    pub admin: Option<(String, String)>,
    /// L'archive du Core (serveur sans internet) ; sinon la release (`version`, la dernière sinon).
    pub package: Option<PathBuf>,
    pub version: Option<String>,
    pub repo: String,
}

impl Default for Plan {
    fn default() -> Self {
        Plan {
            action: Action::Install,
            name: String::new(),
            dns_name: String::new(),
            timezone: "Europe/Paris".into(),
            apps: Vec::new(),
            desktop_video: None,
            ndi: None,
            nvidia: false,
            vaapi: Vec::new(),
            firewall: true,
            admin: None,
            package: None,
            version: None,
            repo: paths::REPO.into(),
        }
    }
}

/// Où et comment agir.
pub struct Ctx {
    pub root: PathBuf,
    log: Mutex<Option<std::fs::File>>,
}

impl Ctx {
    pub fn new(root: &Path) -> Ctx {
        let log_path = under(root, paths::LOG);
        if let Some(d) = log_path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path).ok();
        Ctx { root: root.to_path_buf(), log: Mutex::new(log) }
    }

    /// Un essai à blanc (sous un autre dossier racine).
    pub fn dry(&self) -> bool {
        self.root != Path::new("/")
    }

    pub fn p(&self, path: &str) -> PathBuf {
        under(&self.root, path)
    }

    pub fn log(&self, line: &str) {
        if let Some(f) = self.log.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{} {line}", now());
        }
    }

    /// Une commande du système (écrite au journal ; pas lancée lors d'un essai à blanc).
    pub fn run(&self, prog: &str, args: &[&str]) -> Result<String, String> {
        self.run_input(prog, args, None)
    }

    pub fn run_input(&self, prog: &str, args: &[&str], input: Option<&str>) -> Result<String, String> {
        let line = format!("$ {prog} {}", args.join(" "));
        if self.dry() {
            self.log(&format!("{line}   (essai à blanc : non lancée)"));
            return Ok(String::new());
        }
        self.log(&line);
        exec(self, prog, args, input)
    }

    /// Le Core lui-même (`strata-core …`), sous le compte du service, sur son dossier de données.
    pub fn core(&self, args: &[&str], input: Option<&str>) -> Result<String, String> {
        let bin = self.p(paths::BIN);
        let data = self.p(paths::DATA);
        let mut all: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        all.push("--data-dir".into());
        all.push(data.display().to_string());
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        if self.dry() {
            self.log(&format!("$ {} {}", bin.display(), all.join(" ")));
            return exec(self, &bin.display().to_string(), &all, input);
        }
        let mut with_user = vec!["-u", paths::USER, "--", paths::BIN];
        with_user.extend(all);
        self.log(&format!("$ runuser {}", with_user.join(" ")));
        exec(self, "runuser", &with_user, input)
    }

    /// Une commande du Core qui ne touche pas aux données (`strata-core ptp4l-unit`).
    pub fn core_plain(&self, args: &[&str]) -> Result<String, String> {
        exec(self, &self.p(paths::BIN).display().to_string(), args, None)
    }

    /// Écrit un fichier (ses dossiers créés), avec ses droits.
    pub fn write(&self, path: &str, content: &str, mode: u32) -> Result<(), String> {
        let p = self.p(path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        std::fs::write(&p, content).map_err(|e| format!("cannot write {}: {e}", p.display()))?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode));
        self.log(&format!("wrote {}", p.display()));
        Ok(())
    }

    /// Donne un fichier ou un dossier au compte du service.
    pub fn chown(&self, path: &str, recursive: bool) -> Result<(), String> {
        let owner = format!("{u}:{u}", u = paths::USER);
        let p = self.p(path).display().to_string();
        if recursive {
            self.run("chown", &["-R", &owner, &p]).map(|_| ())
        } else {
            self.run("chown", &[&owner, &p]).map(|_| ())
        }
    }
}

fn exec(ctx: &Ctx, prog: &str, args: &[&str], input: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new(prog);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    cmd.env("DEBIAN_FRONTEND", "noninteractive").env("NEEDRESTART_MODE", "l").env("LC_ALL", "C").env("RUST_BACKTRACE", "0");
    let mut child = cmd.spawn().map_err(|e| format!("{prog}: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    for l in stdout.lines().chain(stderr.lines()) {
        ctx.log(&format!("  {l}"));
    }
    if out.status.success() {
        Ok(stdout)
    } else {
        // Le Core dit « Error: … » ; sinon, la dernière ligne.
        if let Some(e) = stderr.lines().find_map(|l| l.strip_prefix("Error: ")) {
            return Err(e.to_string());
        }
        let last = stderr.lines().chain(stdout.lines()).rfind(|l| !l.trim().is_empty()).unwrap_or("failed").to_string();
        Err(format!("{prog}: {last}"))
    }
}

fn now() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("[{s}]")
}

// ---------- Les fichiers du système ----------

fn core_unit() -> String {
    format!(
        "[Unit]\n\
         Description=Strata Core\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         User={user}\n\
         Group={user}\n\
         SupplementaryGroups=video render\n\
         ExecStart={bin} run --bind 0.0.0.0:{port} --data-dir {data}\n\
         Restart=always\n\
         RestartSec=2\n\
         # Les ports 80 et 443 (cockpit), 161 (SNMP).\n\
         AmbientCapabilities=CAP_NET_BIND_SERVICE\n\
         CapabilityBoundingSet=CAP_NET_BIND_SERVICE\n\
         # SIGTERM au Core, qui arrête ses applis ; SIGKILL au reste après le délai.\n\
         KillMode=mixed\n\
         TimeoutStopSec=20\n\
         LimitNOFILE=65536\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        user = paths::USER,
        bin = paths::BIN,
        port = paths::PORT,
        data = paths::DATA,
    )
}

const POLKIT_RULE: &str = "// Strata Core : le compte du service relance chrony et ptp4l (Réglages › Matériel › Horloge).\n\
polkit.addRule(function (action, subject) {\n\
  if (action.id == \"org.freedesktop.systemd1.manage-units\" && subject.user == \"strata\") {\n\
    var unit = action.lookup(\"unit\");\n\
    if (unit == \"chrony.service\" || unit == \"strata-ptp4l.service\") return polkit.Result.YES;\n\
  }\n\
});\n";

/// Des tampons réseau larges : les flux UDP (ST 2110, NDI, SRT) arrivent en rafales.
const SYSCTL_CONF: &str = "# Strata : tampons réseau pour les flux UDP / RTP / SRT\n\
net.core.rmem_max = 33554432\n\
net.core.rmem_default = 1048576\n\
net.core.wmem_max = 33554432\n\
net.core.netdev_max_backlog = 16384\n";

// ---------- Les étapes ----------

/// Ce que fait une étape : `say` donne l'avancement ; le résultat dit ce qui a été fait.
pub type StepFn = dyn Fn(&Ctx, &dyn Fn(String)) -> Result<String, String> + Send;

/// Une étape : son nom, facultative ou non (un échec n'arrête pas les suivantes), et ce qu'elle fait.
pub struct Step {
    pub label: String,
    pub optional: bool,
    pub run: Box<StepFn>,
}

fn step(label: impl Into<String>, optional: bool, run: impl Fn(&Ctx, &dyn Fn(String)) -> Result<String, String> + Send + 'static) -> Step {
    Step { label: label.into(), optional, run: Box::new(run) }
}

/// Le nom court d'une appli donnée : `strata-cut` pour `DamienP2/strata-cut`, le nom du fichier
/// pour une archive.
fn app_label(src: &str) -> String {
    let last = src.trim_end_matches('/').rsplit('/').next().unwrap_or(src);
    format!("Appli {}", last.split("-linux-").next().unwrap_or(last))
}

fn mb(n: u64) -> String {
    format!("{:.1} Mo", n as f64 / 1e6)
}

/// Pose le Core : depuis l'archive donnée, sinon la release ; l'ancien gardé (`.previous`).
fn put_core(ctx: &Ctx, plan: &Plan, say: &dyn Fn(String)) -> Result<String, String> {
    let tmp = std::env::temp_dir().join(format!("strata-install-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let (version, archive) = match &plan.package {
        Some(p) => ("".to_string(), p.clone()),
        None => release::fetch_core(&plan.repo, plan.version.as_deref(), &tmp, &|done, total| {
            say(if total > 0 { format!("{} / {}", mb(done), mb(total)) } else { mb(done) })
        })?,
    };
    let bin = ctx.p(paths::BIN);
    std::fs::create_dir_all(bin.parent().unwrap()).map_err(|e| e.to_string())?;
    let new = bin.with_extension("new");
    release::extract_core(&archive, &new)?;
    // Il doit démarrer avant de remplacer l'ancien.
    let v = Command::new(&new).arg("--version").output().map_err(|e| format!("the new Core does not start: {e}"))?;
    let shown = String::from_utf8_lossy(&v.stdout).split_whitespace().last().unwrap_or_default().to_string();
    if !v.status.success() || shown.is_empty() {
        let _ = std::fs::remove_file(&new);
        return Err("the new Core does not start".into());
    }
    if bin.exists() {
        let _ = std::fs::copy(&bin, bin.with_extension("previous"));
    }
    std::fs::rename(&new, &bin).map_err(|e| format!("cannot replace {}: {e}", bin.display()))?;
    let link = ctx.p(paths::LINK);
    if let Some(d) = link.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::remove_file(&link);
    let _ = std::os::unix::fs::symlink(paths::BIN, &link);
    ctx.chown(paths::BIN_DIR, true)?;
    let _ = std::fs::remove_dir_all(&tmp);
    ctx.log(&format!("Strata Core {shown} → {}", bin.display()));
    let _ = version;
    Ok(format!("Strata Core {shown}"))
}

fn service_user(ctx: &Ctx) -> Result<String, String> {
    let exists = !ctx.dry() && Command::new("id").args(["-u", paths::USER]).output().is_ok_and(|o| o.status.success());
    if exists {
        // Les GPU et les cartes : groupes video et render.
        let _ = ctx.run("usermod", &["-aG", "video,render", paths::USER]);
        return Ok(format!("compte « {} »", paths::USER));
    }
    ctx.run("useradd", &["--system", "--no-create-home", "--home-dir", paths::DATA, "--shell", "/usr/sbin/nologin", "--groups", "video,render", paths::USER])?;
    Ok(format!("compte « {} » créé", paths::USER))
}

/// Les fichiers du système : service, ptp4l, polkit, réglages que le Core écrit, tampons réseau.
fn system_files(ctx: &Ctx) -> Result<String, String> {
    for d in [paths::DATA, paths::ETC] {
        std::fs::create_dir_all(ctx.p(d)).map_err(|e| format!("cannot create {d}: {e}"))?;
        ctx.chown(d, d == paths::DATA)?;
    }
    ctx.write(paths::UNIT, &core_unit(), 0o644)?;
    let ptp = ctx.core_plain(&["ptp4l-unit"])?;
    if !ptp.trim().is_empty() {
        ctx.write(paths::PTP_UNIT, &ptp, 0o644)?;
    }
    ctx.write(paths::POLKIT, POLKIT_RULE, 0o644)?;
    // Le Core réécrit ce fichier ; il doit le pouvoir.
    if !ctx.p(paths::CHRONY).exists() {
        ctx.write(paths::CHRONY, "# Strata Core : réglé dans Réglages › Matériel › Horloge.\n", 0o644)?;
    }
    ctx.chown(paths::CHRONY, false)?;
    ctx.write(paths::SYSCTL, SYSCTL_CONF, 0o644)?;
    let _ = ctx.run("sysctl", &["-q", "-p", &ctx.p(paths::SYSCTL).display().to_string()]);
    ctx.run("systemctl", &["daemon-reload"])?;
    Ok("service, ptp4l, polkit, tampons réseau".into())
}

/// Le pilote Blackmagic : son paquet sorti de l'archive, installé avec DKMS.
fn desktop_video(ctx: &Ctx, src: &Path) -> Result<String, String> {
    let deb = if src.extension().is_some_and(|e| e == "deb") {
        src.to_path_buf()
    } else {
        let f = std::fs::File::open(src).map_err(|e| format!("cannot open {}: {e}", src.display()))?;
        let gz = src.to_string_lossy().ends_with(".gz");
        let reader: Box<dyn std::io::Read> = if gz { Box::new(flate2::read::GzDecoder::new(f)) } else { Box::new(f) };
        let mut ar = tar::Archive::new(reader);
        let cache = ctx.p("/var/cache/strata");
        std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        let mut found = None;
        for e in ar.entries().map_err(|_| "the archive is damaged (copy it again)".to_string())? {
            let mut e = e.map_err(|_| "the archive is damaged (copy it again)".to_string())?;
            let p = e.path().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
            let name = p.rsplit('/').next().unwrap_or_default().to_string();
            if p.contains("/deb/x86_64/") && name.starts_with("desktopvideo_") && name.ends_with(".deb") && !name.contains("-gui") {
                let out = cache.join(&name);
                let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                std::io::copy(&mut e, &mut w).map_err(|e| e.to_string())?;
                found = Some(out);
            }
        }
        found.ok_or("no desktopvideo package for x86-64 in this archive")?
    };
    let headers = format!("linux-headers-{}", String::from_utf8_lossy(&Command::new("uname").arg("-r").output().map(|o| o.stdout).unwrap_or_default()).trim());
    ctx.run("apt-get", &["install", "-y", "-q", "dkms", &headers])?;
    ctx.run("apt-get", &["install", "-y", "-q", &deb.display().to_string()])?;
    let _ = ctx.run("modprobe", &["blackmagic"]);
    let _ = ctx.run("modprobe", &["blackmagic-io"]);
    let loaded = ctx.dry() || Path::new("/dev/blackmagic").exists();
    Ok(if loaded { "pilote installé (DKMS)".into() } else { "pilote installé : redémarrez le serveur pour le charger".into() })
}

/// La bibliothèque NDI, sortie du SDK (licence acceptée).
fn ndi(ctx: &Ctx, lib: &Path) -> Result<String, String> {
    let name = lib.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let major = name.strip_prefix("libndi.so.").and_then(|r| r.split('.').next()).unwrap_or("6").to_string();
    let target = format!("/usr/local/lib/libndi.so.{major}");
    let dest = ctx.p(&target);
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::copy(lib, &dest).map_err(|e| format!("cannot install {target}: {e}"))?;
    let link = ctx.p("/usr/local/lib/libndi.so");
    let _ = std::fs::remove_file(&link);
    let _ = std::os::unix::fs::symlink(format!("libndi.so.{major}"), &link);
    // La licence et la mention NDI, à côté.
    if let Some(sdk) = lib.ancestors().find(|p| p.file_name().is_some_and(|n| n == "NDI SDK for Linux")) {
        let doc = ctx.p("/usr/local/share/doc/ndi");
        let _ = std::fs::create_dir_all(&doc);
        for e in std::fs::read_dir(sdk).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n.contains("licen") || n.contains("eula") {
                let _ = std::fs::copy(e.path(), doc.join(e.file_name()));
            }
        }
    }
    ctx.run("ldconfig", &[])?;
    ctx.run("apt-get", &["install", "-y", "-q", "avahi-daemon", "libavahi-client3"])?;
    let _ = ctx.run("systemctl", &["enable", "--now", "avahi-daemon"]);
    Ok(format!("{target} ; découverte des sources par Avahi"))
}

/// Le pilote NVIDIA recommandé pour cette carte (celui d'Ubuntu, avec NVENC/NVDEC).
fn nvidia(ctx: &Ctx) -> Result<String, String> {
    ctx.run("apt-get", &["install", "-y", "-q", "ubuntu-drivers-common"])?;
    ctx.run("ubuntu-drivers", &["install"])?;
    let loaded = !ctx.dry() && Command::new("nvidia-smi").output().is_ok_and(|o| o.status.success());
    Ok(if loaded { "pilote NVIDIA en place".into() } else { "pilote installé : redémarrez le serveur pour le charger".into() })
}

fn vaapi(ctx: &Ctx, pkgs: &[String]) -> Result<String, String> {
    let mut args = vec!["install", "-y", "-q"];
    args.extend(pkgs.iter().map(String::as_str));
    ctx.run("apt-get", &args)?;
    Ok(pkgs.join(", "))
}

fn firewall(ctx: &Ctx) -> Result<String, String> {
    let status = if ctx.dry() {
        String::new()
    } else {
        Command::new("ufw").arg("status").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    if !ctx.dry() && !status.contains("Status: active") {
        return Ok("ufw inactif : rien à ouvrir".into());
    }
    ctx.run("ufw", &["allow", "80/tcp", "comment", "Strata Core"])?;
    ctx.run("ufw", &["allow", "443/tcp", "comment", "Strata Core"])?;
    Ok("80/tcp et 443/tcp ouverts".into())
}

fn admin(ctx: &Ctx, name: &str, password: &str) -> Result<String, String> {
    let list = ctx.core(&["user", "list"], None).unwrap_or_default();
    if list.lines().skip(1).any(|l| l.split_whitespace().next() == Some(name)) {
        return Ok(format!("« {name} » existe déjà"));
    }
    ctx.core(&["user", "add", name], Some(&format!("{password}\n{password}\n")))?;
    Ok(format!("« {name} » créé"))
}

/// Le service démarré, et le cockpit qui répond.
fn start(ctx: &Ctx, restart: bool) -> Result<String, String> {
    ctx.run("systemctl", &["enable", "strata-ptp4l"]).ok();
    if restart {
        ctx.run("systemctl", &["enable", "strata-core"])?;
        ctx.run("systemctl", &["restart", "strata-core"])?;
    } else {
        ctx.run("systemctl", &["enable", "--now", "strata-core"])?;
    }
    if ctx.dry() {
        return Ok("service démarré (essai à blanc)".into());
    }
    let until = Instant::now() + Duration::from_secs(40);
    while Instant::now() < until {
        if std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], paths::PORT).into(), Duration::from_millis(500)).is_ok() {
            return Ok(format!("le cockpit répond (port {})", paths::PORT));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("the cockpit does not answer: journalctl -u strata-core".into())
}

/// Les étapes d'un plan.
pub fn steps(plan: &Plan) -> Vec<Step> {
    let mut v: Vec<Step> = Vec::new();
    match plan.action.clone() {
        Action::Install => {
            let mut pkgs = vec!["chrony", "linuxptp", "ethtool", "polkitd"];
            if plan.ndi.is_some() {
                pkgs.push("avahi-daemon");
            }
            let pkgs: Vec<String> = pkgs.into_iter().map(String::from).collect();
            v.push(step("Paquets du système", false, move |ctx, _| {
                ctx.run("apt-get", &["update", "-q"])?;
                let mut args = vec!["install", "-y", "-q"];
                args.extend(pkgs.iter().map(String::as_str));
                ctx.run("apt-get", &args)?;
                Ok(pkgs.join(", "))
            }));
            v.push(step("Compte du service", false, |ctx, _| service_user(ctx)));
            let p = plan.clone();
            v.push(step("Strata Core", false, move |ctx, say| put_core(ctx, &p, say)));
            let (n, d, tz) = (plan.name.clone(), plan.dns_name.clone(), plan.timezone.clone());
            v.push(step("Réglages du serveur", false, move |ctx, _| {
                std::fs::create_dir_all(ctx.p(paths::DATA)).map_err(|e| e.to_string())?;
                let out = ctx.core(&["config", "--name", &n, "--dns-name", &d, "--timezone", &tz], None)?;
                Ok(out.trim().to_string())
            }));
            v.push(step("Fichiers du système", false, |ctx, _| system_files(ctx)));
            if let Some(dv) = plan.desktop_video.clone() {
                v.push(step("Pilote Blackmagic Desktop Video", true, move |ctx, _| desktop_video(ctx, &dv)));
            }
            if let Some(lib) = plan.ndi.clone() {
                v.push(step("Bibliothèque NDI", true, move |ctx, _| ndi(ctx, &lib)));
            }
            if plan.nvidia {
                v.push(step("Pilote NVIDIA", true, |ctx, _| nvidia(ctx)));
            }
            if !plan.vaapi.is_empty() {
                let pkgs = plan.vaapi.clone();
                v.push(step("Pilotes VA-API (Intel, AMD)", true, move |ctx, _| vaapi(ctx, &pkgs)));
            }
            if let Some((name, pw)) = plan.admin.clone() {
                v.push(step(format!("Compte administrateur « {name} »"), false, move |ctx, _| admin(ctx, &name, &pw)));
            }
            for app in plan.apps.clone() {
                v.push(step(app_label(&app), true, move |ctx, _| {
                    ctx.core(&["app", "install", &app, "--activate"], None).map(|o| o.lines().next().unwrap_or_default().to_string())
                }));
            }
            v.push(step("Droits sur les données", false, |ctx, _| ctx.chown(paths::DATA, true).map(|_| paths::DATA.to_string())));
            if plan.firewall {
                v.push(step("Pare-feu", true, |ctx, _| firewall(ctx)));
            }
            v.push(step("Démarrage du service", false, |ctx, _| start(ctx, false)));
        }
        Action::Update => {
            let p = plan.clone();
            v.push(step("Strata Core", false, move |ctx, say| put_core(ctx, &p, say)));
            v.push(step("Fichiers du système", false, |ctx, _| system_files(ctx)));
            for app in plan.apps.clone() {
                v.push(step(app_label(&app), true, move |ctx, _| {
                    ctx.core(&["app", "install", &app, "--activate"], None).map(|o| o.lines().next().unwrap_or_default().to_string())
                }));
            }
            if let Some(dv) = plan.desktop_video.clone() {
                v.push(step("Pilote Blackmagic Desktop Video", true, move |ctx, _| desktop_video(ctx, &dv)));
            }
            if let Some(lib) = plan.ndi.clone() {
                v.push(step("Bibliothèque NDI", true, move |ctx, _| ndi(ctx, &lib)));
            }
            if plan.nvidia {
                v.push(step("Pilote NVIDIA", true, |ctx, _| nvidia(ctx)));
            }
            if !plan.vaapi.is_empty() {
                let pkgs = plan.vaapi.clone();
                v.push(step("Pilotes VA-API (Intel, AMD)", true, move |ctx, _| vaapi(ctx, &pkgs)));
            }
            v.push(step("Droits sur les données", false, |ctx, _| ctx.chown(paths::DATA, true).map(|_| paths::DATA.to_string())));
            v.push(step("Redémarrage du service", false, |ctx, _| start(ctx, true)));
        }
        Action::Repair => {
            v.push(step("Compte du service", false, |ctx, _| service_user(ctx)));
            v.push(step("Fichiers du système", false, |ctx, _| system_files(ctx)));
            v.push(step("Droits", false, |ctx, _| {
                ctx.chown(paths::BIN_DIR, true)?;
                ctx.chown(paths::DATA, true)?;
                Ok("programme et données au compte du service".into())
            }));
            v.push(step("Redémarrage du service", false, |ctx, _| start(ctx, true)));
        }
        Action::Uninstall { purge } => {
            v.push(step("Arrêt du service", false, |ctx, _| {
                let _ = ctx.run("systemctl", &["disable", "--now", "strata-core"]);
                let _ = ctx.run("systemctl", &["disable", "--now", "strata-ptp4l"]);
                Ok("arrêté".into())
            }));
            v.push(step("Fichiers du système", false, |ctx, _| {
                for f in [paths::UNIT, paths::PTP_UNIT, paths::POLKIT, paths::LINK, paths::CHRONY, paths::SYSCTL] {
                    let _ = std::fs::remove_file(ctx.p(f));
                }
                let _ = std::fs::remove_dir_all(ctx.p(paths::ETC));
                let _ = std::fs::remove_dir_all(ctx.p("/opt/strata"));
                let _ = ctx.run("systemctl", &["daemon-reload"]);
                let _ = ctx.run("systemctl", &["restart", "chrony"]);
                Ok("programme et fichiers du système retirés".into())
            }));
            if purge {
                v.push(step("Données", false, |ctx, _| {
                    std::fs::remove_dir_all(ctx.p(paths::DATA))
                        .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
                        .map_err(|e| e.to_string())?;
                    let _ = ctx.run("userdel", &[paths::USER]);
                    Ok(format!("{} effacé", paths::DATA))
                }));
            } else {
                v.push(step("Données", false, |_, _| Ok(format!("gardées dans {}", paths::DATA))));
            }
        }
    }
    v
}

/// Ce qui arrive pendant l'installation.
pub enum Event {
    Start(usize),
    Progress(usize, String),
    Done(usize, Result<String, String>),
    /// Fin : vrai si tout ce qui était nécessaire a marché.
    End(bool),
}

/// Déroule les étapes ; une étape nécessaire qui échoue arrête la suite.
pub fn run(ctx: &Ctx, steps: &[Step], on: &(dyn Fn(Event) + Sync)) -> bool {
    ctx.log("---- strata-install");
    for (i, s) in steps.iter().enumerate() {
        on(Event::Start(i));
        ctx.log(&format!("== {}", s.label));
        let say = |m: String| on(Event::Progress(i, m));
        let r = (s.run)(ctx, &say);
        match &r {
            Ok(m) => ctx.log(&format!("✓ {} : {m}", s.label)),
            Err(e) => ctx.log(&format!("✗ {} : {e}", s.label)),
        }
        let failed = r.is_err() && !s.optional;
        on(Event::Done(i, r));
        if failed {
            on(Event::End(false));
            return false;
        }
    }
    on(Event::End(true));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_have_their_steps() {
        let mut p = Plan {
            name: "Régie A".into(),
            dns_name: "strata01.regie.fr".into(),
            admin: Some(("admin".into(), "password1".into())),
            apps: vec!["DamienP2/strata-cut".into()],
            ..Plan::default()
        };
        let labels: Vec<String> = steps(&p).into_iter().map(|s| s.label).collect();
        assert_eq!(labels.first().map(String::as_str), Some("Paquets du système"));
        assert!(labels.contains(&"Compte administrateur « admin »".to_string()));
        assert!(labels.contains(&"Appli strata-cut".to_string()));
        assert_eq!(app_label("/tmp/demo-1.0.0-linux-x86_64.tar.gz"), "Appli demo-1.0.0");
        assert_eq!(labels.last().map(String::as_str), Some("Démarrage du service"));
        p.action = Action::Uninstall { purge: false };
        assert_eq!(steps(&p).last().map(|s| s.label.clone()).as_deref(), Some("Données"));
        assert!(core_unit().contains("ExecStart=/opt/strata/bin/strata-core run --bind 0.0.0.0:80 --data-dir /var/lib/strata-core"));
    }
}
