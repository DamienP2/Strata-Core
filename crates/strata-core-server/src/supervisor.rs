//! Le lancement des applis actives : le Core démarre chaque appli qui a une section `[run]`, la
//! relance si elle s'arrête (1 s, 2 s, 4 s… 30 s au plus ; 1 s de nouveau après une minute de bonne
//! marche), l'arrête quand on la désactive (SIGTERM à son groupe de processus, SIGKILL après 10 s),
//! et garde ses dernières lignes de journal.
//!
//! Ici, les applis sont des processus enfants du Core : elles s'arrêtent avec lui. L'installateur en
//! fera des services systemd (`strata-app@<id>`), qui survivent à un redémarrage du Core (mise à jour).
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    sync::watch,
    task::JoinHandle,
};

/// Combien de lignes de journal on garde par appli.
const LOG_LINES: usize = 1000;
const LINE_MAX: usize = 2000;
const STOP_GRACE: Duration = Duration::from_secs(10);
const RESTART_MAX: Duration = Duration::from_secs(30);
/// Après ce temps de bonne marche, un arrêt repart d'une seconde d'attente.
const HEALTHY_AFTER: Duration = Duration::from_secs(60);

/// Comment lancer une appli.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// Son port : elle est « en marche » quand il répond.
    pub port: u16,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    /// Pas lancée par le Core.
    #[default]
    Stopped,
    /// Lancée, son port ne répond pas encore.
    Starting,
    /// Lancée, son port répond.
    Running,
    /// Arrêtée d'elle-même : le Core la relance bientôt.
    Crashed,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: RunState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Relances depuis son activation.
    pub restarts: u32,
    /// Depuis quand elle est dans cet état (secondes Unix).
    pub since: u64,
    /// Comment elle s'est arrêtée la dernière fois (« code 3 », « signal 9 », « cannot start: … »).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct LogLine {
    /// Millisecondes Unix.
    pub t: u64,
    /// `out`, `err` (la sortie de l'appli) ou `core` (ce que le Core a fait).
    pub src: &'static str,
    pub text: String,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[derive(Default)]
struct Watch {
    status: Mutex<Status>,
    log: Mutex<VecDeque<LogLine>>,
    /// La tâche qui la garde en marche, et de quoi l'arrêter.
    task: Mutex<Option<Keeper>>,
}

/// L'ordre d'arrêt, la tâche, et comment elle a été lancée.
type Keeper = (watch::Sender<bool>, JoinHandle<()>, Launch);

impl Watch {
    fn set(&self, state: RunState, pid: Option<u32>) {
        let mut st = self.status.lock().unwrap();
        st.state = state;
        st.pid = pid;
        st.since = now_ms() / 1000;
    }

    fn line(&self, src: &'static str, text: &str) {
        let mut text = text.trim_end().to_string();
        if text.len() > LINE_MAX {
            let mut cut = LINE_MAX;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push('…');
        }
        let mut log = self.log.lock().unwrap();
        if log.len() >= LOG_LINES {
            log.pop_front();
        }
        log.push_back(LogLine { t: now_ms(), src, text });
    }
}

/// Les applis lancées par le Core.
#[derive(Default)]
pub struct Supervisor {
    apps: Mutex<HashMap<String, Arc<Watch>>>,
}

impl Supervisor {
    fn watch(&self, id: &str) -> Arc<Watch> {
        self.apps.lock().unwrap().entry(id.to_string()).or_default().clone()
    }

    pub fn status(&self, id: &str) -> Status {
        self.apps.lock().unwrap().get(id).map(|w| w.status.lock().unwrap().clone()).unwrap_or_default()
    }

    pub fn log(&self, id: &str) -> Vec<LogLine> {
        self.apps.lock().unwrap().get(id).map(|w| w.log.lock().unwrap().iter().cloned().collect()).unwrap_or_default()
    }

    /// Les applis que le Core garde en marche.
    pub fn running(&self) -> Vec<String> {
        let apps = self.apps.lock().unwrap();
        let mut ids: Vec<String> = apps.iter().filter(|(_, w)| w.task.lock().unwrap().is_some()).map(|(id, _)| id.clone()).collect();
        ids.sort();
        ids
    }

    /// Lance l'appli et la garde en marche ; déjà lancée de la même façon : rien. Lancée autrement
    /// (une mise à jour a changé sa fiche) : relancée.
    pub async fn start(&self, id: &str, launch: Launch) {
        let w = self.watch(id);
        let same = w.task.lock().unwrap().as_ref().is_some_and(|(_, _, l)| *l == launch);
        if same {
            return;
        }
        self.stop(id).await;
        {
            let mut st = w.status.lock().unwrap();
            st.restarts = 0;
            st.exit = None;
        }
        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(keep(id.to_string(), w.clone(), launch.clone(), rx));
        *w.task.lock().unwrap() = Some((tx, handle, launch));
    }

    /// Arrête l'appli (et attend qu'elle soit arrêtée). Son journal reste.
    pub async fn stop(&self, id: &str) {
        let Some(w) = self.apps.lock().unwrap().get(id).cloned() else { return };
        let task = w.task.lock().unwrap().take();
        if let Some((tx, handle, _)) = task {
            let _ = tx.send(true);
            let _ = handle.await;
        }
    }

    /// Arrête puis relance.
    pub async fn restart(&self, id: &str, launch: Launch) {
        self.stop(id).await;
        self.watch(id).line("core", "restart asked");
        self.start(id, launch).await;
    }

    /// Arrête tout (à l'arrêt du Core).
    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.apps.lock().unwrap().keys().cloned().collect();
        let stops = ids.iter().map(|id| self.stop(id));
        for s in stops {
            s.await;
        }
    }
}

/// Garde l'appli en marche jusqu'à l'ordre d'arrêt.
async fn keep(id: String, w: Arc<Watch>, l: Launch, mut stop: watch::Receiver<bool>) {
    let mut delay = Duration::from_secs(1);
    loop {
        let mut cmd = Command::new(&l.program);
        cmd.args(&l.args).current_dir(&l.cwd).envs(l.env.iter().cloned()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        // Son propre groupe de processus : l'arrêt atteint aussi ses enfants.
        #[cfg(unix)]
        cmd.process_group(0);
        let started = Instant::now();
        match cmd.spawn() {
            Err(e) => {
                let why = format!("cannot start {}: {e}", l.program.display());
                tracing::warn!("{id}: {why}");
                w.line("core", &why);
                w.set(RunState::Crashed, None);
                w.status.lock().unwrap().exit = Some(why);
            }
            Ok(mut child) => {
                let pid = child.id();
                w.set(RunState::Starting, pid);
                w.line("core", &format!("started (pid {})", pid.unwrap_or(0)));
                tracing::info!("{id} started (pid {})", pid.unwrap_or(0));
                if let Some(out) = child.stdout.take() {
                    tokio::spawn(pipe(w.clone(), out, "out"));
                }
                if let Some(err) = child.stderr.take() {
                    tokio::spawn(pipe(w.clone(), err, "err"));
                }
                let mut up = false;
                let exit = loop {
                    tokio::select! {
                        r = child.wait() => break Some(r),
                        _ = stop.changed() => break None,
                        _ = tokio::time::sleep(Duration::from_millis(250)), if !up => {
                            if answers(l.port).await {
                                up = true;
                                w.set(RunState::Running, pid);
                            }
                        }
                    }
                };
                let Some(r) = exit else {
                    terminate(&mut child, pid).await;
                    w.line("core", "stopped");
                    tracing::info!("{id} stopped");
                    w.set(RunState::Stopped, None);
                    return;
                };
                let how = match r {
                    Ok(st) => describe(st),
                    Err(e) => e.to_string(),
                };
                if started.elapsed() > HEALTHY_AFTER {
                    delay = Duration::from_secs(1);
                }
                let msg = format!("exited ({how}); restarting in {} s", delay.as_secs());
                tracing::warn!("{id} {msg}");
                w.line("core", &msg);
                w.set(RunState::Crashed, None);
                w.status.lock().unwrap().exit = Some(how);
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = stop.changed() => {
                w.line("core", "stopped");
                w.set(RunState::Stopped, None);
                return;
            }
        }
        w.status.lock().unwrap().restarts += 1;
        delay = (delay * 2).min(RESTART_MAX);
    }
}

/// Son port répond-il ?
pub async fn answers(port: u16) -> bool {
    matches!(tokio::time::timeout(Duration::from_millis(300), tokio::net::TcpStream::connect(("127.0.0.1", port))).await, Ok(Ok(_)))
}

fn describe(st: std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = st.signal() {
            return format!("signal {sig}");
        }
    }
    match st.code() {
        Some(c) => format!("code {c}"),
        None => "unknown".into(),
    }
}

/// SIGTERM au groupe, SIGKILL après le délai.
async fn terminate(child: &mut Child, pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY : un simple envoi de signal au groupe que l'appli dirige (process_group(0)).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_ok() {
            return;
        }
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
}

async fn pipe(w: Arc<Watch>, from: impl AsyncRead + Unpin, src: &'static str) {
    let mut lines = BufReader::new(from).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        w.line(src, &line);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Launch {
        Launch {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: vec![("HELLO".into(), "bonjour".into())],
            port: 1,
        }
    }

    async fn until(f: impl Fn() -> bool) {
        for _ in 0..100 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out");
    }

    #[tokio::test]
    async fn starts_logs_and_stops() {
        let s = Supervisor::default();
        s.start("demo", sh("echo $HELLO; echo oops >&2; exec sleep 30")).await;
        until(|| s.log("demo").iter().any(|l| l.text == "bonjour") && s.log("demo").iter().any(|l| l.src == "err")).await;
        assert_eq!(s.status("demo").state, RunState::Starting, "port 1 never answers");
        assert!(s.status("demo").pid.is_some());
        assert_eq!(s.running(), ["demo"]);
        s.stop("demo").await;
        assert_eq!(s.status("demo").state, RunState::Stopped);
        assert!(s.running().is_empty());
        assert_eq!(s.log("demo").last().unwrap().text, "stopped");
    }

    #[tokio::test]
    async fn restarts_what_stops() {
        let s = Supervisor::default();
        s.start("crash", sh("echo up; exit 3")).await;
        until(|| s.status("crash").restarts >= 1).await;
        let st = s.status("crash");
        assert_eq!(st.exit.as_deref(), Some("code 3"));
        assert!(s.log("crash").iter().any(|l| l.text.starts_with("exited (code 3)")));
        s.stop("crash").await;
        assert_eq!(s.status("crash").state, RunState::Stopped);
    }

    #[tokio::test]
    async fn cannot_start() {
        let s = Supervisor::default();
        s.start("ghost", Launch { program: "/nonexistent/app".into(), ..sh("") }).await;
        until(|| s.status("ghost").state == RunState::Crashed).await;
        assert!(s.status("ghost").exit.unwrap().starts_with("cannot start"));
        s.stop("ghost").await;
    }
}
