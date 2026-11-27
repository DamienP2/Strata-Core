//! Les ffmpeg des flux réseau : lancés avec un tube de plus (fd 3, le son), tués avec le Core,
//! sans les descripteurs du Core ; leur journal lu ligne à ligne (niveau et avancement).
use std::{
    io::{BufRead, BufReader},
    os::fd::{FromRawFd, OwnedFd, RawFd},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};

/// Le programme : `STRATA_FFMPEG`, sinon `ffmpeg` du système.
pub fn program() -> PathBuf {
    std::env::var_os("STRATA_FFMPEG").filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

/// ffmpeg est-il là ? Sa version (« 6.1.1-3ubuntu5 »).
pub fn version() -> Result<String, String> {
    let out = Command::new(program()).arg("-version").stdin(Stdio::null()).output().map_err(|e| format!("ffmpeg: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next().unwrap_or_default();
    first.split_whitespace().nth(2).map(str::to_string).ok_or_else(|| "ffmpeg did not answer".to_string())
}

/// Le côté du Core du tube fd 3 : lu (sortie de ffmpeg) ou écrit (entrée de ffmpeg).
pub enum Fd3 {
    None,
    /// ffmpeg écrit sur son fd 3, le Core lit.
    Read,
    /// Le Core écrit, ffmpeg lit son fd 3.
    Write,
}

/// Un ffmpeg lancé.
pub struct Proc {
    pub child: Child,
    /// Le tube fd 3, côté Core.
    pub fd3: Option<std::fs::File>,
    /// Ce que ffmpeg dit : avancement, erreurs.
    pub log: Arc<Mutex<Log>>,
}

/// Ce qu'on retient du journal de ffmpeg.
#[derive(Default, Clone, Debug)]
pub struct Log {
    /// L'avancement (`-progress`) : `frame`, `fps`, `bitrate`, `speed`, `drop_frames`…
    pub progress: std::collections::HashMap<String, String>,
    /// Les flux de l'entrée (« Video: h264 (High), yuv420p, 1920x1080, 25 fps »).
    pub streams: Vec<String>,
    /// Les dernières erreurs.
    pub errors: Vec<String>,
    /// Dans la description des sorties (leurs flux ne sont pas ceux de la source).
    in_output: bool,
}

impl Log {
    pub fn last_error(&self) -> Option<String> {
        self.errors.last().cloned()
    }

    pub fn num(&self, key: &str) -> Option<f64> {
        self.progress.get(key).and_then(|v| v.trim().trim_end_matches("kbits/s").trim_end_matches('x').parse().ok())
    }

    fn take(&mut self, line: &str) {
        let line = line.trim_end();
        if let Some((k, v)) = line.split_once('=') {
            if !k.contains(' ') && !k.is_empty() {
                self.progress.insert(k.to_string(), v.to_string());
                return;
            }
        }
        // `-loglevel level+info` : « [info] … », « [error] … ».
        let (level, text) = match line.strip_prefix('[').and_then(|l| l.split_once("] ")) {
            Some((lvl, rest)) if ["info", "error", "fatal", "warning", "panic"].contains(&lvl) => (lvl, rest),
            _ => ("info", line),
        };
        if text.starts_with("Input #") {
            self.in_output = false;
        } else if text.starts_with("Output #") {
            self.in_output = true;
        }
        // Les flux de l'entrée, une fois.
        if let Some(st) = text.trim_start().strip_prefix("Stream #0:") {
            if let Some((_, desc)) = st.split_once(": ") {
                if !self.in_output && self.streams.len() < 8 && (desc.starts_with("Video:") || desc.starts_with("Audio:")) {
                    self.streams.push(desc.to_string());
                }
            }
            return;
        }
        if matches!(level, "error" | "fatal" | "panic") && !text.trim().is_empty() {
            // Sans le préfixe « [tcp @ 0x…] ».
            let clean = match text.strip_prefix('[').and_then(|t| t.split_once("] ")) {
                Some((_, rest)) => rest,
                None => text,
            };
            self.errors.push(clean.trim().to_string());
            if self.errors.len() > 5 {
                self.errors.remove(0);
            }
        }
    }
}

fn pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: two descriptors written by the kernel, owned once returned.
    let r = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if r != 0 {
        return Err(format!("pipe: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: fresh descriptors from pipe2.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Lance ffmpeg : stdin et stdout en tubes si demandé, fd 3 selon `fd3`, stderr lu.
pub fn spawn(args: &[String], stdin: bool, stdout: bool, fd3: Fd3) -> Result<Proc, String> {
    let mut cmd = Command::new(program());
    cmd.args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "level+info", "-progress", "pipe:2", "-stats_period", "1"]);
    cmd.args(args);
    cmd.stdin(if stdin { Stdio::piped() } else { Stdio::null() });
    cmd.stdout(if stdout { Stdio::piped() } else { Stdio::null() });
    cmd.stderr(Stdio::piped());
    // Le tube fd 3 : l'extrémité de ffmpeg passe en 3, celle du Core reste ici.
    let (mine, theirs): (Option<OwnedFd>, Option<OwnedFd>) = match fd3 {
        Fd3::None => (None, None),
        Fd3::Read => {
            let (r, w) = pipe()?;
            (Some(r), Some(w))
        }
        Fd3::Write => {
            let (r, w) = pipe()?;
            (Some(w), Some(r))
        }
    };
    let theirs_raw = theirs.as_ref().map(std::os::fd::AsRawFd::as_raw_fd);
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut cmd, move || {
            // Tué avec le Core.
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            if let Some(fd) = theirs_raw {
                // Déjà le 3 : dup2 ne fait rien, mais il faut retirer O_CLOEXEC.
                let r = if fd == 3 { libc::fcntl(3, libc::F_SETFD, 0) } else { libc::dup2(fd, 3) };
                if r < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            // Aucun droit du Core pour ffmpeg (il lit des flux venus du réseau).
            libc::prctl(crate::supervisor::PR_CAP_AMBIENT, crate::supervisor::PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0);
            // Rien d'autre du Core.
            const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
            let _ = libc::syscall(libc::SYS_close_range, 4 as libc::c_uint, libc::c_uint::MAX, CLOSE_RANGE_CLOEXEC);
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot start ffmpeg ({}): {e}", program().display()))?;
    drop(theirs);
    let log = Arc::new(Mutex::new(Log::default()));
    if let Some(err) = child.stderr.take() {
        let log = log.clone();
        let _ = std::thread::Builder::new().name("ffmpeg-log".into()).spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                log.lock().unwrap().take(&line);
            }
        });
    }
    Ok(Proc { child, fd3: mine.map(std::fs::File::from), log })
}

impl Proc {
    /// Encore en marche ?
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_ffmpeg_says() {
        let mut l = Log::default();
        for line in [
            "[info] Input #0, mpegts, from 'srt://10.0.0.1:9000':",
            "[info]   Stream #0:0[0x100]: Video: h264 (High) ([27][0][0][0] / 0x001B), yuv420p(tv, bt709, progressive), 1920x1080, 25 fps, 25 tbr, 90k tbn",
            "[info]   Stream #0:1[0x101](fra): Audio: aac (LC) ([15][0][0][0] / 0x000F), 48000 Hz, stereo, fltp, 192 kb/s",
            "[info] Output #0, rawvideo, to 'pipe:1':",
            "[info]   Stream #0:0: Video: v210, yuv422p10le, 1920x1080, q=2-31, 829440 kb/s, 25 fps",
            "frame=125",
            "fps=25.0",
            "bitrate=6012.3kbits/s",
            "speed=1.00x",
            "[error] [srt @ 0x55d] Connection to srt://10.0.0.1:9000 failed: Connection refused",
        ] {
            l.take(line);
        }
        assert_eq!(l.streams.len(), 2);
        assert!(l.streams[0].starts_with("Video: h264 (High)"));
        assert_eq!(l.num("fps"), Some(25.0));
        assert_eq!(l.num("bitrate"), Some(6012.3));
        assert_eq!(l.num("speed"), Some(1.0));
        assert_eq!(l.last_error().unwrap(), "Connection to srt://10.0.0.1:9000 failed: Connection refused");
    }
}
