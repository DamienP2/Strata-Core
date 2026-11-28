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

    /// Une commande longue (apt, pilotes) : son avancement montré au fil de l'eau (`say`).
    pub fn run_live(&self, prog: &str, args: &[&str], say: &dyn Fn(String)) -> Result<String, String> {
        use std::io::BufRead;
        let line = format!("$ {prog} {}", args.join(" "));
        if self.dry() {
            self.log(&format!("{line}   (essai à blanc : non lancée)"));
            return Ok(String::new());
        }
        self.log(&line);
        let mut child = Command::new(prog)
            .args(args)
            .env("DEBIAN_FRONTEND", "noninteractive")
            .env("NEEDRESTART_MODE", "l")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{prog}: {e}"))?;
        let err = child.stderr.take().map(|e| {
            std::thread::spawn(move || {
                let mut s = String::new();
                let _ = std::io::Read::read_to_string(&mut { e }, &mut s);
                s
            })
        });
        let mut last = String::new();
        if let Some(out) = child.stdout.take() {
            for l in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                self.log(&format!("  {l}"));
                if let Some(p) = progress_of(&l) {
                    say(p);
                }
                if !l.trim().is_empty() {
                    last = l;
                }
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        let stderr = err.and_then(|h| h.join().ok()).unwrap_or_default();
        for l in stderr.lines() {
            self.log(&format!("  {l}"));
        }
        if status.success() {
            Ok(last)
        } else {
            let why = stderr.lines().chain(std::iter::once(last.as_str())).rfind(|l| !l.trim().is_empty()).unwrap_or("failed").to_string();
            Err(format!("{prog}: {why}"))
        }
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
        // Le Core tourne sous son compte : le dossier des données doit être à lui avant qu'il y
        // écrive (créé ici par root à la première installation).
        std::fs::create_dir_all(&data).map_err(|e| format!("cannot create {}: {e}", data.display()))?;
        self.chown(paths::DATA, false)?;
        let mut with_user = vec!["-u", paths::USER, "--", paths::BIN];
        with_user.extend(all);
        self.log(&format!("$ runuser {}", with_user.join(" ")));
        exec(self, "runuser", &with_user, input)
    }

    /// Une commande du Core qui ne touche pas aux données (`strata-core ptp4l-unit`).
    pub fn core_plain(&self, args: &[&str]) -> Result<String, String> {
        if self.dry() {
            return exec(self, &self.p(paths::BIN).display().to_string(), args, None);
        }
        // Le programme est au compte du service : root ne le lance pas lui-même.
        let mut with_user = vec!["-u", paths::USER, "--", paths::BIN];
        with_user.extend(args);
        exec(self, "runuser", &with_user, None)
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
        if let Some(e) = core_error(&stderr) {
            return Err(e);
        }
        let last = stderr.lines().chain(stdout.lines()).rfind(|l| !l.trim().is_empty()).unwrap_or("failed").to_string();
        Err(format!("{prog}: {last}"))
    }
}

/// Une ligne d'apt (ou de DKMS) dite en clair, pour l'avancement d'une étape.
fn progress_of(l: &str) -> Option<String> {
    let pkg = |rest: &str| rest.split_whitespace().next().unwrap_or_default().split(':').next().unwrap_or_default().to_string();
    if l.starts_with("Get:") {
        // « Get:12 http://archive.ubuntu.com/ubuntu noble/restricted amd64 nvidia-driver-580 amd64 … »
        let w: Vec<&str> = l.split_whitespace().collect();
        return w.get(4).map(|p| format!("téléchargement : {p}"));
    }
    if let Some(r) = l.strip_prefix("Unpacking ") {
        return Some(format!("décompression : {}", pkg(r)));
    }
    if let Some(r) = l.strip_prefix("Setting up ") {
        return Some(format!("installation : {}", pkg(r)));
    }
    if l.contains("Building initial module") || l.contains("Building module") || l.starts_with("Building for ") {
        return Some("compilation du module du noyau (quelques minutes)…".into());
    }
    if l.starts_with("update-initramfs") || l.contains("Generating /boot/initrd") {
        return Some("préparation du démarrage (initramfs)…".into());
    }
    None
}

/// L'erreur du Core : « Error: … », puis ses causes (« Caused by: »), sur une ligne.
fn core_error(stderr: &str) -> Option<String> {
    let e = stderr.lines().find_map(|l| l.strip_prefix("Error: "))?;
    let causes: Vec<&str> = stderr
        .lines()
        .skip_while(|l| !l.starts_with("Caused by:"))
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        // « 0: … », « 1: … » quand il y en a plusieurs.
        .map(|l| l.split_once(": ").filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit())).map_or(l, |(_, r)| r))
        .collect();
    Some(if causes.is_empty() { e.to_string() } else { format!("{e}: {}", causes.join(": ")) })
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
         # Le dossier partagé des images (MXL) au compte du service, même créé par root (sudo).\n\
         ExecStartPre=+/usr/bin/install -d -o {user} -g {user} -m 0755 /dev/shm/mxl\n\
         ExecStartPre=+/usr/bin/chown -R {user}:{user} /dev/shm/mxl\n\
         ExecStart={bin} run --bind 0.0.0.0:{port} --data-dir {data}\n\
         Restart=always\n\
         RestartSec=2\n\
         # Ses fichiers (journaux, réglages, applis) : à lui seul.\n\
         UMask=0077\n\
         # Les ports 80 et 443 (cockpit), 161 (SNMP) ; le ST 2110 par MTL : sockets brutes et\n\
         # AF_XDP (NET_RAW, NET_ADMIN, BPF), mémoire verrouillée (IPC_LOCK), priorités (SYS_NICE).\n\
         AmbientCapabilities=CAP_NET_BIND_SERVICE CAP_NET_RAW CAP_NET_ADMIN CAP_BPF CAP_IPC_LOCK CAP_SYS_NICE\n\
         CapabilityBoundingSet=CAP_NET_BIND_SERVICE CAP_NET_RAW CAP_NET_ADMIN CAP_BPF CAP_IPC_LOCK CAP_SYS_NICE\n\
         LimitMEMLOCK=infinity\n\
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

const POLKIT_RULE: &str = "// Strata Core : le compte du service relance chrony et ptp4l (Matériel › Horloge), et\n\
// strata-media (hugepages et ports DPDK du ST 2110, Matériel › Réseau).\n\
polkit.addRule(function (action, subject) {\n\
  if (action.id == \"org.freedesktop.systemd1.manage-units\" && subject.user == \"strata\") {\n\
    var unit = action.lookup(\"unit\");\n\
    var verb = action.lookup(\"verb\");\n\
    var ok = verb == \"start\" || verb == \"stop\" || verb == \"restart\";\n\
    if (ok && (unit == \"chrony.service\" || unit == \"strata-ptp4l.service\" || unit == \"strata-media.service\")) return polkit.Result.YES;\n\
  }\n\
});\n";

/// Des tampons réseau larges : les flux UDP (ST 2110, NDI, SRT) arrivent en rafales.
const SYSCTL_CONF: &str = "# Strata : tampons réseau pour les flux UDP / RTP / SRT\n\
net.core.rmem_max = 33554432\n\
net.core.rmem_default = 1048576\n\
net.core.wmem_max = 33554432\n\
net.core.netdev_max_backlog = 16384\n";

/// Ce dont MTL et DPDK (livrés avec le Core, ST 2110) ont besoin sur le système.
const MTL_PKGS: [&str; 5] = ["libnuma1", "libjson-c5", "libxdp1", "libbpf1", "libelf1t64"];

/// Les paquets du système dont le Core a besoin : l'heure (chrony, ptp4l), ethtool, polkit,
/// ffmpeg (les flux réseau) et ce qu'il faut à MTL (ST 2110).
fn system_pkgs(ndi: bool) -> Vec<String> {
    let mut pkgs = vec!["chrony", "linuxptp", "ethtool", "polkitd", "ffmpeg"];
    pkgs.extend(MTL_PKGS);
    if ndi {
        pkgs.push("avahi-daemon");
    }
    pkgs.into_iter().map(String::from).collect()
}

/// Le dépôt « universe » d'Ubuntu (ffmpeg, linuxptp y sont) : ajouté aux sources s'il n'y est pas.
fn enable_universe(ctx: &Ctx) -> Result<bool, String> {
    let deb822 = ctx.p("/etc/apt/sources.list.d/ubuntu.sources");
    if let Ok(text) = std::fs::read_to_string(&deb822) {
        if text.lines().filter(|l| l.starts_with("Components:")).all(|l| l.split_whitespace().any(|c| c == "universe")) {
            return Ok(false);
        }
        let fixed: Vec<String> = text
            .lines()
            .map(|l| if l.starts_with("Components:") && !l.split_whitespace().any(|c| c == "universe") { format!("{l} universe") } else { l.to_string() })
            .collect();
        ctx.write("/etc/apt/sources.list.d/ubuntu.sources", &(fixed.join("\n") + "\n"), 0o644)?;
        return Ok(true);
    }
    // L'ancienne forme (sources.list).
    ctx.run("add-apt-repository", &["-y", "universe"]).map(|_| true)
}

/// Installe les paquets du système, puis vérifie que ffmpeg répond.
fn system_packages(ctx: &Ctx, pkgs: &[String], say: &dyn Fn(String)) -> Result<String, String> {
    say("mise à jour de la liste des paquets…".into());
    ctx.run("apt-get", &["update", "-q"])?;
    if !ctx.dry() {
        let candidate =
            Command::new("apt-cache").args(["policy", "ffmpeg"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        if !candidate.contains("Candidate:") || candidate.contains("Candidate: (none)") {
            say("ajout du dépôt universe d'Ubuntu (ffmpeg)…".into());
            if enable_universe(ctx)? {
                ctx.run("apt-get", &["update", "-q"])?;
            }
        }
    }
    let mut args = vec!["install", "-y", "-q"];
    args.extend(pkgs.iter().map(String::as_str));
    ctx.run_live("apt-get", &args, say)?;
    if !ctx.dry() {
        let ok = Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
        if !ok {
            return Err("ffmpeg does not run after its installation (apt-get install ffmpeg)".into());
        }
    }
    Ok(pkgs.join(", "))
}

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
    let tmp = crate::checks::private_tmp("strata-install")?;
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
    // MXL (les flux vidéo partagés avec les applis), livrée dans l'archive du Core.
    // Le dossier existe toujours et est au compte du service : une mise à jour par le Core y pose
    // les bibliothèques de la version suivante.
    std::fs::create_dir_all(ctx.p(paths::LIB_DIR)).map_err(|e| e.to_string())?;
    release::extract_libs(&archive, &ctx.p(paths::LIB_DIR))?;
    ctx.chown(paths::LIB_DIR, true)?;
    let link = ctx.p(paths::LINK);
    if let Some(d) = link.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::remove_file(&link);
    let _ = std::os::unix::fs::symlink(paths::BIN, &link);
    ctx.chown(paths::BIN_DIR, true)?;
    put_media_helper(ctx, &bin)?;
    let _ = std::fs::remove_dir_all(&tmp);
    ctx.log(&format!("Strata Core {shown} → {}", bin.display()));
    let _ = version;
    Ok(format!("Strata Core {shown}"))
}

/// La copie du Core que lance `strata-media` (root) : à root, 0755, dans un dossier à root.
fn put_media_helper(ctx: &Ctx, bin: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let dir = ctx.p(paths::HELPER_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
    let dst = ctx.p(paths::MEDIA_HELPER);
    let tmp = dst.with_extension("new");
    let _ = std::fs::remove_file(&tmp);
    std::fs::copy(bin, &tmp).map_err(|e| format!("cannot copy the Core to {}: {e}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    if !ctx.dry() {
        ctx.run("chown", &["root:root", &tmp.display().to_string()])?;
    }
    std::fs::rename(&tmp, &dst).map_err(|e| format!("cannot replace {}: {e}", dst.display()))?;
    Ok(())
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
    for d in [paths::DATA, paths::ETC, paths::LIB_DIR] {
        std::fs::create_dir_all(ctx.p(d)).map_err(|e| format!("cannot create {d}: {e}"))?;
        ctx.chown(d, d != paths::ETC)?;
    }
    // Les données (secrets, journaux avec les adresses des flux) : au seul compte du service.
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(ctx.p(paths::DATA), std::fs::Permissions::from_mode(0o700));
    }
    ctx.write(paths::UNIT, &core_unit(), 0o644)?;
    let ptp = ctx.core_plain(&["ptp4l-unit"])?;
    if !ptp.trim().is_empty() {
        ctx.write(paths::PTP_UNIT, &ptp, 0o644)?;
    }
    ctx.write(paths::POLKIT, POLKIT_RULE, 0o644)?;
    // Un Core plus ancien n'a pas ce service.
    let media = ctx.core_plain(&["media-unit"]).unwrap_or_default();
    if !media.trim().is_empty() {
        ctx.write(paths::MEDIA_UNIT, &media, 0o644)?;
        if ctx.dry() || ctx.p(paths::MEDIA_HELPER).exists() {
            let _ = ctx.run("systemctl", &["enable", "strata-media"]);
        } else {
            // Une installation d'avant la copie à root : la mise à jour la pose.
            ctx.log(&format!("{} is missing: run the update to set up ST 2110 (DPDK, hugepages)", paths::MEDIA_HELPER));
        }
    }
    // Le Core réécrit ce fichier ; il doit le pouvoir.
    if !ctx.p(paths::CHRONY).exists() {
        ctx.write(paths::CHRONY, "# Strata Core : réglé dans Réglages › Matériel › Horloge.\n", 0o644)?;
    }
    ctx.chown(paths::CHRONY, false)?;
    ctx.write(paths::SYSCTL, SYSCTL_CONF, 0o644)?;
    let _ = ctx.run("sysctl", &["-q", "-p", &ctx.p(paths::SYSCTL).display().to_string()]);
    ctx.run("systemctl", &["daemon-reload"])?;
    Ok("service, ptp4l, ST 2110, polkit, tampons réseau".into())
}

/// Le pilote Blackmagic : son paquet sorti de l'archive, installé avec DKMS.
fn desktop_video(ctx: &Ctx, src: &Path, say: &dyn Fn(String)) -> Result<String, String> {
    // Ce qui a été copié : l'archive de Blackmagic (.tar.gz, .tar), son dossier déjà déballé, le
    // paquet .deb, ou un installateur .sh / .run.
    let dir = if src.is_dir() {
        src.to_path_buf()
    } else if is_archive(src) {
        let out = ctx.p("/var/cache/strata/desktopvideo");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        say("décompression de l'archive Blackmagic…".into());
        unpack_any(src, &out)?;
        // Une archive dans l'archive (un .tar.gz dans le .tar) : déballée à son tour.
        for inner in files_in(&out).into_iter().filter(|f| is_archive(f)) {
            let dest = inner.with_extension("d");
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            unpack_any(&inner, &dest)?;
        }
        out
    } else {
        src.parent().unwrap_or(Path::new("/")).to_path_buf()
    };
    let files = if src.is_file() && !is_archive(src) { vec![src.to_path_buf()] } else { files_in(&dir) };
    let headers = format!("linux-headers-{}", String::from_utf8_lossy(&Command::new("uname").arg("-r").output().map(|o| o.stdout).unwrap_or_default()).trim());
    ctx.run_live("apt-get", &["install", "-y", "-q", "dkms", &headers], say)?;
    if let Some(deb) = pick_deb(&files) {
        ctx.run_live("apt-get", &["install", "-y", "-q", &deb.display().to_string()], say)?;
    } else if let Some(sh) = pick_installer(&files) {
        // L'installateur de Blackmagic : lancé dans son dossier, chaque question acceptée.
        let cwd = sh.parent().unwrap_or(Path::new("/"));
        // Les chemins passés en paramètres ($1, $2), jamais collés dans la commande.
        let (cwd, sh) = (cwd.display().to_string(), sh.display().to_string());
        ctx.run("sh", &["-c", "cd \"$1\" && yes | sh \"$2\"", "sh", &cwd, &sh])?;
    } else {
        let mut names: Vec<String> = files.iter().map(|f| f.strip_prefix(&dir).unwrap_or(f).display().to_string()).collect();
        names.sort();
        names.truncate(12);
        return Err(format!("no Desktop Video package (.deb) nor installer (.sh) found; it holds: {}", names.join(", ")));
    }
    // L'archive déballée (2 Go pour Desktop Video 16) n'a plus d'usage.
    if dir.starts_with(ctx.p("/var/cache/strata")) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let _ = ctx.run("modprobe", &["blackmagic"]);
    let _ = ctx.run("modprobe", &["blackmagic-io"]);
    let loaded = ctx.dry() || Path::new("/dev/blackmagic").exists();
    Ok(if loaded { "pilote installé (DKMS)".into() } else { "pilote installé : redémarrez le serveur pour le charger".into() })
}

/// Déballe une archive tar, compressée (gzip) ou non, d'après son contenu et pas son nom (un
/// navigateur peut garder « .tar » à un fichier compressé, ou l'inverse).
fn unpack_any(src: &Path, out: &Path) -> Result<(), String> {
    let mut head = [0u8; 262];
    let n = {
        use std::io::Read;
        let mut f = std::fs::File::open(src).map_err(|e| format!("cannot open {}: {e}", src.display()))?;
        f.read(&mut head).map_err(|e| e.to_string())?
    };
    let size = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    let what = match &head[..n.min(6)] {
        [0x1f, 0x8b, ..] => "gzip",
        [b'B', b'Z', b'h', ..] => return Err("this archive is compressed with bzip2: copy the .tar or .tar.gz from Blackmagic".into()),
        [0xfd, b'7', b'z', b'X', b'Z', 0] => return Err("this archive is compressed with xz: copy the .tar or .tar.gz from Blackmagic".into()),
        [b'P', b'K', ..] => return Err("this is a .zip: copy the Linux .tar or .tar.gz from Blackmagic".into()),
        _ if n >= 262 && &head[257..262] == b"ustar" => "tar",
        _ => return Err(format!("{} is not a tar archive ({size} bytes): copy it again, completely", src.display())),
    };
    let f = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let reader: Box<dyn std::io::Read> = if what == "gzip" { Box::new(flate2::read::GzDecoder::new(f)) } else { Box::new(f) };
    tar::Archive::new(reader).unpack(out).map_err(|e| {
        format!("cannot unpack {} ({size} bytes, {e}): copy it again, completely", src.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())
    })
}

fn is_archive(p: &Path) -> bool {
    let n = p.to_string_lossy().to_lowercase();
    n.ends_with(".tar.gz") || n.ends_with(".tgz") || n.ends_with(".tar")
}

/// Les fichiers d'un dossier, sous-dossiers compris.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                todo.push(p);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

fn lower_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Le paquet du pilote pour x86-64 (`desktopvideo_14.4.1a4_amd64.deb`), sans l'interface graphique.
fn pick_deb(files: &[PathBuf]) -> Option<PathBuf> {
    let mut v: Vec<&PathBuf> = files
        .iter()
        .filter(|p| {
            let n = lower_name(p);
            n.starts_with("desktopvideo") && n.ends_with(".deb") && !n.contains("gui") && !n.contains("arm64") && !n.contains("aarch64")
        })
        .collect();
    v.sort();
    v.pop().cloned()
}

/// Un installateur `.sh` / `.run` (pas un désinstallateur), pour x86-64 ; celui qui parle de
/// Desktop Video ou d'installation d'abord.
fn pick_installer(files: &[PathBuf]) -> Option<PathBuf> {
    let mut v: Vec<(bool, &PathBuf)> = files
        .iter()
        .filter(|p| {
            let n = lower_name(p);
            (n.ends_with(".sh") || n.ends_with(".run")) && !n.contains("uninstall") && !n.contains("arm64") && !n.contains("aarch64")
        })
        .map(|p| {
            let n = lower_name(p).replace(['_', '-', ' '], "");
            (n.contains("desktopvideo") || n.contains("install"), p)
        })
        .collect();
    v.sort();
    v.pop().map(|(_, p)| p.clone())
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
fn nvidia(ctx: &Ctx, say: &dyn Fn(String)) -> Result<String, String> {
    say("recherche du pilote recommandé…".into());
    ctx.run_live("apt-get", &["install", "-y", "-q", "ubuntu-drivers-common"], say)?;
    ctx.run_live("ubuntu-drivers", &["install"], say)?;
    let loaded = !ctx.dry() && Command::new("nvidia-smi").output().is_ok_and(|o| o.status.success());
    Ok(if loaded { "pilote NVIDIA en place".into() } else { "pilote installé : redémarrez le serveur pour le charger".into() })
}

fn vaapi(ctx: &Ctx, pkgs: &[String], say: &dyn Fn(String)) -> Result<String, String> {
    let mut args = vec!["install", "-y", "-q"];
    args.extend(pkgs.iter().map(String::as_str));
    ctx.run_live("apt-get", &args, say)?;
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
    // NMOS : l'API du nœud (IS-04, IS-05) et son annonce mDNS, pour le contrôleur de la régie.
    ctx.run("ufw", &["allow", "3212/tcp", "comment", "Strata Core NMOS"])?;
    ctx.run("ufw", &["allow", "5353/udp", "comment", "Strata Core NMOS (mDNS)"])?;
    // SNMP (Santé › SNMP), s'il est activé.
    ctx.run("ufw", &["allow", "161/udp", "comment", "Strata Core SNMP"])?;
    Ok("80/tcp, 443/tcp, 3212/tcp (NMOS), 5353/udp (mDNS) et 161/udp (SNMP) ouverts".into())
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
            let pkgs = system_pkgs(plan.ndi.is_some());
            v.push(step("Paquets du système", false, move |ctx, say| system_packages(ctx, &pkgs, say)));
            v.push(step("Compte du service", false, |ctx, _| service_user(ctx)));
            let p = plan.clone();
            v.push(step("Strata Core", false, move |ctx, say| put_core(ctx, &p, say)));
            let (n, d, tz) = (plan.name.clone(), plan.dns_name.clone(), plan.timezone.clone());
            v.push(step("Réglages du serveur", false, move |ctx, _| {
                let out = ctx.core(&["config", "--name", &n, "--dns-name", &d, "--timezone", &tz], None)?;
                Ok(out.trim().to_string())
            }));
            v.push(step("Fichiers du système", false, |ctx, _| system_files(ctx)));
            if let Some(dv) = plan.desktop_video.clone() {
                v.push(step("Pilote Blackmagic Desktop Video", true, move |ctx, say| desktop_video(ctx, &dv, say)));
            }
            if let Some(lib) = plan.ndi.clone() {
                v.push(step("Bibliothèque NDI", true, move |ctx, _| ndi(ctx, &lib)));
            }
            if plan.nvidia {
                v.push(step("Pilote NVIDIA", true, |ctx, say| nvidia(ctx, say)));
            }
            if !plan.vaapi.is_empty() {
                let pkgs = plan.vaapi.clone();
                v.push(step("Pilotes VA-API (Intel, AMD)", true, move |ctx, say| vaapi(ctx, &pkgs, say)));
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
            // Un Core plus récent peut demander de nouveaux paquets (ffmpeg, MTL…).
            let pkgs = system_pkgs(plan.ndi.is_some());
            v.push(step("Paquets du système", true, move |ctx, say| system_packages(ctx, &pkgs, say)));
            let p = plan.clone();
            v.push(step("Strata Core", false, move |ctx, say| put_core(ctx, &p, say)));
            v.push(step("Fichiers du système", false, |ctx, _| system_files(ctx)));
            for app in plan.apps.clone() {
                v.push(step(app_label(&app), true, move |ctx, _| {
                    ctx.core(&["app", "install", &app, "--activate"], None).map(|o| o.lines().next().unwrap_or_default().to_string())
                }));
            }
            if let Some(dv) = plan.desktop_video.clone() {
                v.push(step("Pilote Blackmagic Desktop Video", true, move |ctx, say| desktop_video(ctx, &dv, say)));
            }
            if let Some(lib) = plan.ndi.clone() {
                v.push(step("Bibliothèque NDI", true, move |ctx, _| ndi(ctx, &lib)));
            }
            if plan.nvidia {
                v.push(step("Pilote NVIDIA", true, |ctx, say| nvidia(ctx, say)));
            }
            if !plan.vaapi.is_empty() {
                let pkgs = plan.vaapi.clone();
                v.push(step("Pilotes VA-API (Intel, AMD)", true, move |ctx, say| vaapi(ctx, &pkgs, say)));
            }
            v.push(step("Droits sur les données", false, |ctx, _| ctx.chown(paths::DATA, true).map(|_| paths::DATA.to_string())));
            v.push(step("Redémarrage du service", false, |ctx, _| start(ctx, true)));
        }
        Action::Repair => {
            let pkgs = system_pkgs(plan.ndi.is_some());
            v.push(step("Paquets du système", true, move |ctx, say| system_packages(ctx, &pkgs, say)));
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
                // Les ports DPDK rendus au noyau, les hugepages rendues.
                if ctx.p(paths::MEDIA_UNIT).exists() {
                    let _ = ctx.write("/etc/strata/media.json", "{\"hugepages\": 0, \"dpdk\": []}\n", 0o644);
                    if ctx.p(paths::MEDIA_HELPER).exists() {
                        let _ = ctx.run(paths::MEDIA_HELPER, &["media-setup"]);
                    }
                    let _ = ctx.run("systemctl", &["disable", "strata-media"]);
                }
                Ok("arrêté".into())
            }));
            v.push(step("Fichiers du système", false, |ctx, _| {
                for f in [paths::UNIT, paths::PTP_UNIT, paths::MEDIA_UNIT, paths::POLKIT, paths::LINK, paths::CHRONY, paths::SYSCTL] {
                    let _ = std::fs::remove_file(ctx.p(f));
                }
                let _ = std::fs::remove_dir_all(ctx.p(paths::ETC));
                let _ = std::fs::remove_dir_all(ctx.p("/opt/strata"));
                let _ = std::fs::remove_dir_all(ctx.p(paths::HELPER_DIR));
                let _ = ctx.run("systemctl", &["daemon-reload"]);
                let _ = ctx.run("systemctl", &["restart", "chrony"]);
                Ok("programme et fichiers du système retirés".into())
            }));
            if purge {
                v.push(step("Données", false, |ctx, _| {
                    std::fs::remove_dir_all(ctx.p(paths::DATA))
                        .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
                        .map_err(|e| e.to_string())?;
                    let _ = std::fs::remove_dir_all(ctx.p("/var/cache/strata"));
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

    #[test]
    fn apt_lines_said_plainly() {
        assert_eq!(
            progress_of("Get:12 http://archive.ubuntu.com/ubuntu noble/restricted amd64 nvidia-driver-580 amd64 580.95-0ubuntu1 [490 kB]").unwrap(),
            "téléchargement : nvidia-driver-580"
        );
        assert_eq!(progress_of("Unpacking libnvidia-encode-580:amd64 (580.95-0ubuntu1) ...").unwrap(), "décompression : libnvidia-encode-580");
        assert_eq!(progress_of("Setting up nvidia-dkms-580 (580.95-0ubuntu1) ...").unwrap(), "installation : nvidia-dkms-580");
        assert!(progress_of("Building initial module nvidia/580.95 for 6.8.0-49-generic").unwrap().contains("module du noyau"));
        assert!(progress_of("Reading package lists...").is_none());
    }

    #[test]
    fn live_commands_tell_their_progress() {
        // Pour de vrai (racine « / »), mais sans journal : rien n'est écrit dans /var/log.
        let ctx = Ctx { root: "/".into(), log: Mutex::new(None) };
        let said = std::sync::Mutex::new(Vec::new());
        let out = ctx.run_live("sh", &["-c", "echo 'Setting up a (1) ...'; echo 'Unpacking b (2) ...'; echo fin"], &|s| said.lock().unwrap().push(s)).unwrap();
        assert_eq!(out, "fin");
        assert_eq!(*said.lock().unwrap(), vec!["installation : a".to_string(), "décompression : b".to_string()]);
        assert!(ctx.run_live("sh", &["-c", "echo boom >&2; exit 3"], &|_| {}).unwrap_err().contains("boom"));
    }

    #[test]
    fn unpacks_tar_whatever_its_name() {
        let base = std::env::temp_dir().join(format!("strata-dv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src/Blackmagic_Desktop_Video_Linux_15.0/deb/x86_64");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("desktopvideo_15.0a1_amd64.deb"), b"deb").unwrap();
        let mut tar_bytes = Vec::new();
        {
            let mut t = tar::Builder::new(&mut tar_bytes);
            t.append_dir_all("Blackmagic_Desktop_Video_Linux_15.0", base.join("src/Blackmagic_Desktop_Video_Linux_15.0")).unwrap();
            t.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
        let gz = gz.finish().unwrap();
        // Un .tar qui est en fait compressé, et un vrai .tar.
        for (name, bytes) in [("a.tar", &gz), ("b.tar", &tar_bytes), ("c.tar.gz", &gz)] {
            let f = base.join(name);
            std::fs::write(&f, bytes).unwrap();
            let out = base.join(format!("{name}.out"));
            unpack_any(&f, &out).unwrap();
            assert!(pick_deb(&files_in(&out)).is_some(), "{name}");
        }
        // Coupé en route (copie pas finie) : l'erreur le dit.
        let cut = base.join("cut.tar");
        std::fs::write(&cut, &tar_bytes[..tar_bytes.len() / 2]).unwrap();
        let e = unpack_any(&cut, &base.join("cut.out"));
        assert!(e.is_ok() || e.unwrap_err().contains("copy it again"));
        std::fs::write(base.join("z.tar"), b"PK\x03\x04 not a tar").unwrap();
        assert!(unpack_any(&base.join("z.tar"), &base.join("z.out")).unwrap_err().contains(".zip"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn finds_the_blackmagic_driver_in_what_was_copied() {
        let p = |v: &[&str]| v.iter().map(PathBuf::from).collect::<Vec<_>>();
        let debs = p(&[
            "x/deb/x86_64/desktopvideo-gui_14.4_amd64.deb",
            "x/deb/x86_64/desktopvideo_14.4_amd64.deb",
            "x/deb/arm64/desktopvideo_14.4_arm64.deb",
            "x/mediaexpress_3.8_amd64.deb",
        ]);
        assert_eq!(pick_deb(&debs).unwrap(), PathBuf::from("x/deb/x86_64/desktopvideo_14.4_amd64.deb"));
        let shs = p(&["x/uninstall.sh", "x/README.sh", "x/Blackmagic_Desktop_Video_Linux_15.0_installer.sh"]);
        assert_eq!(pick_installer(&shs).unwrap(), PathBuf::from("x/Blackmagic_Desktop_Video_Linux_15.0_installer.sh"));
        assert!(pick_deb(&shs).is_none());
        assert!(is_archive(Path::new("/root/Blackmagic_Desktop_Video_Linux_15.0.TAR.GZ")) && !is_archive(Path::new("a.sh")));
    }

    #[test]
    fn core_errors_keep_their_cause() {
        assert_eq!(
            core_error("Error: cannot save config.json\n\nCaused by:\n    Permission denied (os error 13)\n").unwrap(),
            "cannot save config.json: Permission denied (os error 13)"
        );
        assert_eq!(core_error("Error: a\n\nCaused by:\n    0: b\n    1: c\n").unwrap(), "a: b: c");
        assert_eq!(core_error("Error: alone\n").unwrap(), "alone");
        assert!(core_error("panic").is_none());
    }
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
