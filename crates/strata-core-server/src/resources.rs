//! Les ressources du serveur (Réglages › Santé › Ressources) : processeurs, mémoire, disques,
//! réseau, ce que consomment le Core et chaque appli, ce que la machine peut encore prendre, et
//! les 10 dernières minutes. Relevé toutes les 2 s par un fil à part.
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use serde_json::{json, Value};
use sysinfo::{Disks, Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::{
    api::{require, ApiResult},
    auth::CurrentUser,
    logs::now_ms,
    state::{AppState, Shared},
};

/// Points gardés : 10 minutes toutes les 2 s.
const HISTORY: usize = 300;
const EVERY: Duration = Duration::from_secs(2);

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub t: i64,
    /// % de tous les processeurs.
    pub cpu: f32,
    /// % de la mémoire.
    pub mem: f32,
    /// bit/s, toutes les cartes réseau.
    pub rx: f64,
    pub tx: f64,
    /// Cœurs pris par le Core et chaque appli.
    pub apps: BTreeMap<String, f32>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Disk {
    pub mount: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    /// Celui des données du Core.
    pub data: bool,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Net {
    pub name: String,
    /// bit/s.
    pub rx: f64,
    pub tx: f64,
}

/// Ce que prend un programme (le Core, ou une appli et ses enfants).
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Use {
    /// `core` ou l'id de l'appli.
    pub id: String,
    /// Cœurs (1,5 = un cœur et demi).
    pub cores: f32,
    /// Octets.
    pub memory: u64,
    pub processes: usize,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Now {
    pub cpu: f32,
    pub per_core: Vec<f32>,
    pub load: [f64; 3],
    pub mem_used: u64,
    pub mem_total: u64,
    pub mem_available: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub disks: Vec<Disk>,
    pub nets: Vec<Net>,
    pub uses: Vec<Use>,
}

struct Inner {
    sys: System,
    disks: Disks,
    nets: Networks,
    at: Instant,
    now: Now,
    history: VecDeque<Point>,
}

/// Les relevés.
pub struct Resources {
    inner: Mutex<Option<Inner>>,
}

impl Default for Resources {
    fn default() -> Self {
        Resources { inner: Mutex::new(None) }
    }
}

const SKIP_FS: [&str; 8] = ["tmpfs", "devtmpfs", "squashfs", "overlay", "efivarfs", "ramfs", "nsfs", "fuse.snapfuse"];

impl Resources {
    /// Un relevé (bloquant) ; les applis : (id, pid de leur processus principal).
    pub fn sample(&self, apps: &[(String, u32)], data_dir: &std::path::Path) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let first = g.is_none();
        let inner = g.get_or_insert_with(|| Inner {
            sys: System::new(),
            disks: Disks::new_with_refreshed_list(),
            nets: Networks::new_with_refreshed_list(),
            at: Instant::now(),
            now: Now::default(),
            history: VecDeque::new(),
        });
        let dt = inner.at.elapsed().as_secs_f64().max(0.001);
        inner.at = Instant::now();
        let sys = &mut inner.sys;
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());
        inner.disks.refresh(true);
        inner.nets.refresh(true);

        // Ce que prend chaque programme : un processus et ses descendants.
        let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
        for (pid, p) in sys.processes() {
            if let Some(parent) = p.parent() {
                children.entry(parent).or_default().push(*pid);
            }
        }
        let tree = |root: Pid, deep: bool| -> Use {
            let mut u = Use::default();
            let mut todo = vec![root];
            while let Some(pid) = todo.pop() {
                if let Some(p) = sys.process(pid) {
                    u.cores += p.cpu_usage() / 100.0;
                    u.memory += p.memory();
                    u.processes += 1;
                }
                if deep {
                    todo.extend(children.get(&pid).into_iter().flatten().copied());
                }
            }
            u.cores = (u.cores * 100.0).round() / 100.0;
            u
        };
        let mut uses = vec![Use { id: "core".into(), ..tree(Pid::from_u32(std::process::id()), false) }];
        for (id, pid) in apps {
            uses.push(Use { id: id.clone(), ..tree(Pid::from_u32(*pid), true) });
        }

        let data = data_dir.canonicalize().unwrap_or_else(|_| data_dir.to_path_buf());
        let mut disks: Vec<Disk> = Vec::new();
        for d in inner.disks.list() {
            let fs = d.file_system().to_string_lossy().into_owned();
            let mount = d.mount_point().to_string_lossy().into_owned();
            if SKIP_FS.contains(&fs.as_str())
                || mount.starts_with("/snap")
                || mount.starts_with("/boot/efi")
                || disks.iter().any(|x| x.mount == mount)
                || d.total_space() == 0
            {
                continue;
            }
            disks.push(Disk { mount, fs, total: d.total_space(), free: d.available_space(), data: false });
        }
        // Le disque des données : le point de montage le plus long qui les contient.
        if let Some(i) = disks.iter().enumerate().filter(|(_, d)| data.starts_with(&d.mount)).max_by_key(|(_, d)| d.mount.len()).map(|(i, _)| i) {
            disks[i].data = true;
        }
        disks.sort_by(|a, b| b.data.cmp(&a.data).then(a.mount.cmp(&b.mount)));

        let mut nets: Vec<Net> = inner
            .nets
            .iter()
            .filter(|(n, _)| *n != "lo" && !n.starts_with("veth") && !n.starts_with("docker"))
            .map(|(n, d)| Net { name: n.clone(), rx: (d.received() as f64 * 8.0 / dt).round(), tx: (d.transmitted() as f64 * 8.0 / dt).round() })
            .collect();
        nets.sort_by(|a, b| a.name.cmp(&b.name));

        let load = System::load_average();
        let now = Now {
            cpu: (sys.global_cpu_usage() * 10.0).round() / 10.0,
            per_core: sys.cpus().iter().map(|c| c.cpu_usage().round()).collect(),
            load: [load.one, load.five, load.fifteen],
            mem_used: sys.total_memory().saturating_sub(sys.available_memory()),
            mem_total: sys.total_memory(),
            mem_available: sys.available_memory(),
            swap_used: sys.used_swap(),
            swap_total: sys.total_swap(),
            disks,
            nets,
            uses,
        };
        // Le premier relevé n'a pas encore de durée pour les processeurs et le réseau.
        if !first {
            let p = Point {
                t: now_ms(),
                cpu: now.cpu,
                mem: if now.mem_total > 0 { (now.mem_used as f64 / now.mem_total as f64 * 1000.0).round() as f32 / 10.0 } else { 0.0 },
                rx: now.nets.iter().map(|n| n.rx).sum(),
                tx: now.nets.iter().map(|n| n.tx).sum(),
                apps: now.uses.iter().map(|u| (u.id.clone(), u.cores)).collect(),
            };
            if inner.history.len() == HISTORY {
                inner.history.pop_front();
            }
            inner.history.push_back(p);
        }
        inner.now = now;
    }

    pub fn now(&self) -> Option<Now> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|i| i.now.clone())
    }

    pub fn history(&self) -> Vec<Point> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|i| i.history.iter().cloned().collect()).unwrap_or_default()
    }
}

/// Les applis en marche : (id, pid).
fn running_apps(s: &AppState) -> Vec<(String, u32)> {
    s.supervisor.running().into_iter().filter_map(|id| s.supervisor.status(&id).pid.map(|p| (id, p))).collect()
}

/// Relève toutes les 2 s tant que le Core tourne.
pub fn start(s: &Shared) {
    let weak: Weak<AppState> = Arc::downgrade(s);
    std::thread::Builder::new()
        .name("resources".into())
        .spawn(move || loop {
            let Some(s) = weak.upgrade() else { return };
            s.resources.sample(&running_apps(&s), &s.data_dir);
            drop(s);
            std::thread::sleep(EVERY);
        })
        .ok();
}

/// La machine, en quelques lignes (l'export pour le support).
pub fn system_text(s: &AppState) -> String {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    let cfg = s.config.load();
    let apps: Vec<String> =
        s.apps.list().iter().map(|a| format!("{} {}{}", a.manifest.id, a.manifest.version, if a.active { " (active)" } else { "" })).collect();
    let (cards, dv, _) = crate::hardware::decklink::detect();
    format!(
        "Strata Core {}\nServer: {} ({})\nOS: {} {} · kernel {}\nCPU: {} × {}\nMemory: {} MB (available {} MB)\nCore up: {} s · machine up: {} s\nApps: {}\nDeckLink: {} · Desktop Video {}\nArchive made: {}\n",
        strata_core::VERSION,
        cfg.name,
        System::host_name().unwrap_or_default(),
        System::name().unwrap_or_default(),
        System::os_version().unwrap_or_default(),
        System::kernel_version().unwrap_or_default(),
        sys.cpus().len(),
        sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default(),
        sys.total_memory() / 1_048_576,
        sys.available_memory() / 1_048_576,
        crate::health::uptime_secs(),
        System::uptime(),
        if apps.is_empty() { "none".into() } else { apps.join(", ") },
        if cards.is_empty() { "none".into() } else { cards.iter().map(|c| c.model_name.clone()).collect::<Vec<_>>().join(", ") },
        dv.unwrap_or_else(|| "—".into()),
        jiff::Timestamp::now(),
    )
}

/// Les ressources maintenant, les 10 dernières minutes, et la machine.
async fn resources(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    let v = crate::api::blocking(move || {
        if s2.resources.now().is_none() {
            // Pas encore de relevé (le fil démarre) : deux, à 300 ms d'écart.
            s2.resources.sample(&running_apps(&s2), &s2.data_dir);
            std::thread::sleep(Duration::from_millis(300));
            s2.resources.sample(&running_apps(&s2), &s2.data_dir);
        }
        let names: BTreeMap<String, String> = s2.apps.list().into_iter().map(|a| (a.manifest.id.clone(), a.manifest.name.clone())).collect();
        let mut sys = System::new();
        sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
        json!({
            "now": s2.resources.now(),
            "history": s2.resources.history(),
            "names": names,
            "gpus": crate::hardware::gpu::detect().into_iter().filter(|g| g.usage.is_some()).collect::<Vec<_>>(),
            "machine": {
                "host": System::host_name(),
                "os": format!("{} {}", System::name().unwrap_or_default(), System::os_version().unwrap_or_default()),
                "kernel": System::kernel_version(),
                "cpu": sys.cpus().first().map(|c| c.brand().trim().to_string()),
                "cores": sys.cpus().len(),
                "uptimeSecs": System::uptime(),
                "coreUptimeSecs": crate::health::uptime_secs(),
            },
        })
    })
    .await?;
    Ok(Json(v))
}

pub fn routes() -> Router<Shared> {
    Router::new().route("/api/health/resources", get(resources))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_the_machine_and_this_process() {
        let r = Resources::default();
        let dir = std::env::temp_dir();
        r.sample(&[], &dir);
        std::thread::sleep(Duration::from_millis(250));
        r.sample(&[("self".into(), std::process::id())], &dir);
        let now = r.now().unwrap();
        assert!(now.mem_total > 0 && !now.per_core.is_empty());
        assert_eq!(now.uses[0].id, "core");
        assert_eq!(now.uses[1].id, "self");
        assert!(now.uses[1].memory > 0);
        assert_eq!(r.history().len(), 1);
    }
}
