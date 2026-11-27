//! Une entrée vidéo du serveur, captée une seule fois et lue par plusieurs programmes.
//!
//! Le pilote DeckLink ne laisse ouvrir une entrée qu'à un seul programme. Le Core capte donc
//! chaque entrée réglée sur « Entrée » et la publie dans **une mémoire partagée** (un fichier de
//! `/dev/shm`) ; chaque appli (LiveCut, Multiview…) l'ouvre en lecture avec [`Reader`].
//!
//! - **Un anneau de quelques images** : l'écrivain remplit la case suivante pendant que les
//!   lecteurs lisent la plus récente. Chaque case est gardée par un **verrou de séquence** : un
//!   compteur impair pendant l'écriture ; un lecteur qui le voit changer recommence. Une image lue
//!   est donc toujours entière.
//! - **Réveil** : chaque image publiée réveille les lecteurs qui attendent (futex sous Linux) ; pas
//!   besoin de scruter.
//! - **Lecteurs inscrits** : chaque lecteur s'inscrit avec le nom de son appli (`STRATA_APP_ID`) ;
//!   le Core montre ainsi « Lu par LiveCut ». Une inscription dont le processus a disparu est
//!   effacée.
//! - **Remplacement** : l'écrivain crée toujours un nouveau fichier (écrit à côté puis renommé) ; il
//!   ne retaille jamais celui qu'un lecteur a ouvert. Un lecteur dont l'écrivain s'est arrêté, ou dont
//!   le fichier a été remplacé, le voit ([`Reader::gone`]) et rouvre.
//!
//! Format des images : UYVY 8 bits (4:2:2), lignes contiguës (`stride` octets par ligne).
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{fence, AtomicU32, AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAGIC: [u8; 8] = *b"STRSHM01";
const VERSION: u32 = 1;
/// Le nombre maximal de lecteurs inscrits sur une entrée.
pub const MAX_READERS: usize = 16;
/// Le nombre maximal de cases de l'anneau.
pub const MAX_SLOTS: usize = 8;
/// Assez pour une image UHD (3840 × 2160) en UYVY. Le fichier est creux : seules les pages écrites
/// prennent de la mémoire.
pub const UHD_UYVY: usize = 3840 * 2160 * 2;

const STATE_RUNNING: u32 = 1;
const STATE_CLOSED: u32 = 2;

/// Le dossier des entrées partagées : `STRATA_SHARE_DIR`, sinon `/dev/shm`.
pub fn dir() -> PathBuf {
    match std::env::var_os("STRATA_SHARE_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from("/dev/shm"),
    }
}

/// Le fichier d'une entrée SDI : `strata-sdi-<carte>-<n>` (n : le numéro logiciel, SDI n).
pub fn sdi_path(card: &str, connector: u32) -> PathBuf {
    let card: String = card.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '_' }).collect();
    dir().join(format!("strata-sdi-{card}-{connector}"))
}

// ---------- La disposition en mémoire ----------

#[repr(C)]
struct Header {
    magic: [u8; 8],
    version: u32,
    slots: u32,
    slot_bytes: u64,
    data_offset: u64,
    writer_pid: AtomicU32,
    state: AtomicU32,
    /// 0 : rien encore reçu ; 1 : un signal ; 2 : pas de signal.
    signal: AtomicU32,
    /// Le mot du futex : augmente à chaque image (ou changement d'état).
    wake: AtomicU32,
    /// Le numéro de la dernière image complète (0 : aucune ; les images sont numérotées dès 1).
    latest: AtomicU64,
    /// Le dernier signe de vie de l'écrivain (ms depuis 1970).
    beat: AtomicU64,
    _reserved: [u64; 8],
    readers: [ReaderSlot; MAX_READERS],
    metas: [Meta; MAX_SLOTS],
}

#[repr(C)]
struct ReaderSlot {
    pid: AtomicU32,
    _pad: u32,
    /// Sa dernière lecture (ms depuis 1970).
    seen: AtomicU64,
    /// Les images qu'il a lues.
    frames: AtomicU64,
    /// Le nom de son appli (UTF-8, complété de zéros), écrit avant `pid`… puis relu tel quel.
    app: [AtomicU64; 5],
}

#[repr(C)]
struct Meta {
    /// Le verrou de séquence : impair pendant l'écriture de la case.
    seq: AtomicU64,
    no: AtomicU64,
    width: AtomicU32,
    height: AtomicU32,
    stride: AtomicU32,
    fourcc: AtomicU32,
    rate_num: AtomicU32,
    rate_den: AtomicU32,
    /// bit 0 : entrelacé ; bit 1 : timecode présent ; bit 2 : drop frame ; bit 3 : field mark.
    flags: AtomicU32,
    /// h << 24 | m << 16 | s << 8 | f.
    timecode: AtomicU32,
    captured_ns: AtomicU64,
    len: AtomicU64,
}

const PAGE: usize = 4096;

fn data_offset() -> usize {
    std::mem::size_of::<Header>().div_ceil(PAGE) * PAGE
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn now_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}

fn alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

// ---------- Les images ----------

/// Le format des pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 8 bits 4:2:2, Cb Y0 Cr Y1.
    Uyvy,
}

impl Format {
    fn fourcc(self) -> u32 {
        u32::from_le_bytes(*b"UYVY")
    }
    fn from_fourcc(v: u32) -> Option<Self> {
        (v == u32::from_le_bytes(*b"UYVY")).then_some(Format::Uyvy)
    }
}

/// Un timecode (RP188 ou VITC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub drop_frame: bool,
    /// La seconde image d'une paire (au-delà de 30 images/s).
    pub field_mark: bool,
}

/// Ce qui décrit une image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    /// Octets par ligne.
    pub stride: u32,
    pub format: Format,
    /// La cadence, en images par seconde (25/1, 30000/1001…).
    pub rate: (u32, u32),
    pub interlaced: bool,
    pub timecode: Option<Timecode>,
}

/// Une image lue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Son numéro, depuis le début de la capture (1, 2, 3…) : un saut veut dire des images manquées.
    pub no: u64,
    pub info: FrameInfo,
    /// L'heure de sa capture (ns depuis 1970).
    pub captured_ns: u64,
}

/// Un lecteur inscrit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderInfo {
    pub pid: u32,
    /// Le nom de son appli.
    pub app: String,
    /// Sa dernière lecture (ms depuis 1970).
    pub seen_ms: u64,
    pub frames: u64,
}

// ---------- La mémoire ----------

struct Map {
    ptr: *mut u8,
    len: usize,
    /// (appareil, inode) du fichier.
    ino: (u64, u64),
}

// SAFETY: la mémoire est partagée par nature ; tout accès concurrent passe par des atomiques ou
// par le verrou de séquence.
unsafe impl Send for Map {}
unsafe impl Sync for Map {}

impl Map {
    fn open(file: &fs::File, len: usize) -> io::Result<Map> {
        use std::os::unix::{fs::MetadataExt, io::AsRawFd};
        let meta = file.metadata()?;
        // SAFETY: plain mmap of an open file; checked below.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, file.as_raw_fd(), 0) };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Map { ptr: ptr.cast(), len, ino: (meta.dev(), meta.ino()) })
    }

    fn header(&self) -> &Header {
        // SAFETY: the mapping is at least `data_offset()` long and page-aligned.
        unsafe { &*(self.ptr as *const Header) }
    }

    fn slot(&self, i: usize) -> *mut u8 {
        let h = self.header();
        // SAFETY: inside the mapping (checked at open).
        unsafe { self.ptr.add(h.data_offset as usize + i * h.slot_bytes as usize) }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: mapped by `open` with this length.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

#[cfg(target_os = "linux")]
fn futex_wait(word: &AtomicU32, expected: u32, timeout: Duration) {
    let ts = libc::timespec { tv_sec: timeout.as_secs() as libc::time_t, tv_nsec: timeout.subsec_nanos() as libc::c_long };
    // SAFETY: a shared futex on a word of the mapping (no PRIVATE flag: other processes wake it).
    unsafe { libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAIT, expected, &ts as *const libc::timespec) };
}

#[cfg(target_os = "linux")]
fn futex_wake(word: &AtomicU32) {
    // SAFETY: see futex_wait.
    unsafe { libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAKE, i32::MAX) };
}

#[cfg(not(target_os = "linux"))]
fn futex_wait(word: &AtomicU32, expected: u32, timeout: Duration) {
    let end = Instant::now() + timeout;
    while word.load(Ordering::Acquire) == expected && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(not(target_os = "linux"))]
fn futex_wake(_: &AtomicU32) {}

fn set_mode(file: &fs::File) {
    use std::os::unix::fs::PermissionsExt;
    // Les applis lancées par le Core tournent sous le même compte ; un groupe commun suffira plus tard.
    let _ = file.set_permissions(fs::Permissions::from_mode(0o660));
}

// ---------- L'écrivain ----------

/// L'écrivain d'une entrée (le Core). Le fichier disparaît avec lui.
pub struct Writer {
    map: Map,
    path: PathBuf,
    no: u64,
}

impl Writer {
    /// Crée l'entrée `path` : `slots` cases (2 à 8) de `slot_bytes` octets au plus. Un fichier déjà
    /// là (un Core arrêté net) est remplacé, sans gêner ses lecteurs.
    pub fn create(path: &Path, slots: usize, slot_bytes: usize) -> io::Result<Writer> {
        if !(2..=MAX_SLOTS).contains(&slots) || slot_bytes == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "2 to 8 slots, of some bytes"));
        }
        let slot_bytes = slot_bytes.div_ceil(PAGE) * PAGE;
        let len = data_offset() + slots * slot_bytes;
        let tmp = path.with_file_name(format!(".{}.{}", path.file_name().map_or("share".into(), |n| n.to_string_lossy()), std::process::id()));
        let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&tmp)?;
        set_mode(&file);
        file.set_len(len as u64)?;
        let map = Map::open(&file, len)?;
        // SAFETY: fresh, zeroed, private to us until renamed.
        unsafe {
            let h = map.ptr as *mut Header;
            (*h).magic = MAGIC;
            (*h).version = VERSION;
            (*h).slots = slots as u32;
            (*h).slot_bytes = slot_bytes as u64;
            (*h).data_offset = data_offset() as u64;
        }
        let h = map.header();
        h.writer_pid.store(std::process::id(), Ordering::Relaxed);
        h.beat.store(now_ms(), Ordering::Relaxed);
        h.state.store(STATE_RUNNING, Ordering::Release);
        if let Err(e) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(Writer { map, path: path.to_path_buf(), no: 0 })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// La taille maximale d'une image.
    pub fn slot_bytes(&self) -> usize {
        self.map.header().slot_bytes as usize
    }

    /// Publie une image : `data` (au plus [`Self::slot_bytes`]) ; son numéro.
    pub fn publish(&mut self, info: &FrameInfo, data: &[u8]) -> io::Result<u64> {
        self.publish_with(info, data.len(), |dst| dst.copy_from_slice(data))
    }

    /// Publie une image écrite sur place par `fill` (`len` octets), sans copie intermédiaire.
    pub fn publish_with(&mut self, info: &FrameInfo, len: usize, fill: impl FnOnce(&mut [u8])) -> io::Result<u64> {
        let h = self.map.header();
        if len > h.slot_bytes as usize {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("a frame of {len} bytes does not fit ({} at most)", h.slot_bytes)));
        }
        let no = self.no + 1;
        let i = (no % h.slots as u64) as usize;
        let m = &h.metas[i];
        let seq = m.seq.load(Ordering::Relaxed);
        m.seq.store(seq | 1, Ordering::Relaxed);
        fence(Ordering::Release);
        // SAFETY: the slot is `slot_bytes` long; readers that read it meanwhile see `seq` change.
        fill(unsafe { std::slice::from_raw_parts_mut(self.map.slot(i), len) });
        m.no.store(no, Ordering::Relaxed);
        m.width.store(info.width, Ordering::Relaxed);
        m.height.store(info.height, Ordering::Relaxed);
        m.stride.store(info.stride, Ordering::Relaxed);
        m.fourcc.store(info.format.fourcc(), Ordering::Relaxed);
        m.rate_num.store(info.rate.0, Ordering::Relaxed);
        m.rate_den.store(info.rate.1, Ordering::Relaxed);
        let tc = info.timecode;
        let flags = info.interlaced as u32
            | (tc.is_some() as u32) << 1
            | (tc.is_some_and(|t| t.drop_frame) as u32) << 2
            | (tc.is_some_and(|t| t.field_mark) as u32) << 3;
        m.flags.store(flags, Ordering::Relaxed);
        m.timecode.store(tc.map_or(0, |t| (t.hours as u32) << 24 | (t.minutes as u32) << 16 | (t.seconds as u32) << 8 | t.frames as u32), Ordering::Relaxed);
        m.captured_ns.store(now_ns(), Ordering::Relaxed);
        m.len.store(len as u64, Ordering::Relaxed);
        m.seq.store((seq | 1) + 1, Ordering::Release);
        self.no = no;
        h.latest.store(no, Ordering::Release);
        h.signal.store(1, Ordering::Relaxed);
        h.beat.store(now_ms(), Ordering::Relaxed);
        h.wake.fetch_add(1, Ordering::Release);
        futex_wake(&h.wake);
        Ok(no)
    }

    /// Le signal : présent, absent, ou rien reçu encore.
    pub fn set_signal(&self, signal: Option<bool>) {
        let h = self.map.header();
        let v = match signal {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        };
        if h.signal.swap(v, Ordering::Relaxed) != v {
            h.wake.fetch_add(1, Ordering::Release);
            futex_wake(&h.wake);
        }
        h.beat.store(now_ms(), Ordering::Relaxed);
    }

    /// Un signe de vie, quand aucune image ne vient.
    pub fn beat(&self) {
        self.map.header().beat.store(now_ms(), Ordering::Relaxed);
    }

    /// Les lecteurs inscrits (ceux dont le processus a disparu sont effacés).
    pub fn readers(&self) -> Vec<ReaderInfo> {
        readers_of(self.map.header(), true)
    }

    /// Le nombre d'images publiées.
    pub fn published(&self) -> u64 {
        self.no
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let h = self.map.header();
        h.state.store(STATE_CLOSED, Ordering::Release);
        h.wake.fetch_add(1, Ordering::Release);
        futex_wake(&h.wake);
        // Le fichier n'est retiré que s'il est encore le nôtre.
        if same_file(&self.path, self.map.ino) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn same_file(path: &Path, ino: (u64, u64)) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).is_ok_and(|m| (m.dev(), m.ino()) == ino)
}

fn read_app(slot: &ReaderSlot) -> String {
    let mut bytes = Vec::with_capacity(40);
    for w in &slot.app {
        bytes.extend_from_slice(&w.load(Ordering::Relaxed).to_le_bytes());
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn readers_of(h: &Header, clean: bool) -> Vec<ReaderInfo> {
    let mut out = Vec::new();
    for r in &h.readers {
        let pid = r.pid.load(Ordering::Acquire);
        if pid == 0 {
            continue;
        }
        if !alive(pid) {
            if clean {
                let _ = r.pid.compare_exchange(pid, 0, Ordering::AcqRel, Ordering::Relaxed);
            }
            continue;
        }
        out.push(ReaderInfo { pid, app: read_app(r), seen_ms: r.seen.load(Ordering::Relaxed), frames: r.frames.load(Ordering::Relaxed) });
    }
    out
}

// ---------- Le lecteur ----------

/// Ce que donne [`Reader::next`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Une image nouvelle, dans le tampon.
    Frame(Frame),
    /// Rien de nouveau dans le délai.
    Timeout,
    /// L'écrivain s'est arrêté ou le fichier a été remplacé : rouvrir.
    Gone,
}

/// Un lecteur d'une entrée (une appli). Désinscrit quand il disparaît.
pub struct Reader {
    map: Map,
    path: PathBuf,
    slot: Option<usize>,
    last: u64,
}

impl Reader {
    /// Ouvre l'entrée `path` et s'inscrit avec le nom `app` (celui de l'appli : `STRATA_APP_ID`).
    pub fn open(path: &Path, app: &str) -> io::Result<Reader> {
        let mut r = Self::observe(path)?;
        r.slot = r.register(app);
        if r.slot.is_none() {
            return Err(io::Error::other(format!("{} has {MAX_READERS} readers already", path.display())));
        }
        Ok(r)
    }

    /// Ouvre l'entrée sans s'inscrire (le Core, pour un aperçu).
    pub fn observe(path: &Path) -> io::Result<Reader> {
        let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len() as usize;
        if len < data_offset() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a Strata share"));
        }
        let map = Map::open(&file, len)?;
        let h = map.header();
        if h.magic != MAGIC || h.version != VERSION || h.slots as usize > MAX_SLOTS || h.data_offset as usize + h.slots as usize * h.slot_bytes as usize > len {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a Strata share (or another version)"));
        }
        Ok(Reader { map, path: path.to_path_buf(), slot: None, last: 0 })
    }

    fn register(&self, app: &str) -> Option<usize> {
        let h = self.map.header();
        let me = std::process::id();
        let mut name = [0u8; 40];
        let b = app.as_bytes();
        // Coupé sur une limite de caractère.
        let mut n = b.len().min(39);
        while !app.is_char_boundary(n) {
            n -= 1;
        }
        name[..n].copy_from_slice(&b[..n]);
        for (i, r) in h.readers.iter().enumerate() {
            let pid = r.pid.load(Ordering::Acquire);
            let free = pid == 0 || !alive(pid);
            if free && r.pid.compare_exchange(pid, me, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                for (k, w) in r.app.iter().enumerate() {
                    w.store(u64::from_le_bytes(name[k * 8..k * 8 + 8].try_into().unwrap()), Ordering::Relaxed);
                }
                r.seen.store(now_ms(), Ordering::Relaxed);
                r.frames.store(0, Ordering::Relaxed);
                return Some(i);
            }
        }
        None
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Le signal : présent, absent, ou rien reçu encore.
    pub fn signal(&self) -> Option<bool> {
        match self.map.header().signal.load(Ordering::Relaxed) {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        }
    }

    /// L'écrivain s'est arrêté (ou a disparu), ou le fichier a été remplacé : il faut rouvrir.
    pub fn gone(&self) -> bool {
        let h = self.map.header();
        h.state.load(Ordering::Acquire) == STATE_CLOSED || !alive(h.writer_pid.load(Ordering::Relaxed)) || !same_file(&self.path, self.map.ino)
    }

    /// Le dernier signe de vie de l'écrivain (ms depuis 1970).
    pub fn beat_ms(&self) -> u64 {
        self.map.header().beat.load(Ordering::Relaxed)
    }

    /// Les lecteurs inscrits.
    pub fn readers(&self) -> Vec<ReaderInfo> {
        readers_of(self.map.header(), false)
    }

    /// La plus récente image, si elle est plus récente que la dernière lue : copiée dans `buf`.
    pub fn latest(&mut self, buf: &mut Vec<u8>) -> Option<Frame> {
        let mut out = None;
        self.latest_with(|f, data| {
            buf.clear();
            buf.extend_from_slice(data);
            out = Some(f);
        })?;
        out
    }

    /// La plus récente image, lue sur place par `read` (qui peut être appelé plusieurs fois si
    /// l'écrivain repasse sur la case pendant la lecture ; seul le dernier appel compte).
    pub fn latest_with(&mut self, mut read: impl FnMut(Frame, &[u8])) -> Option<Frame> {
        let h = self.map.header();
        for _ in 0..8 {
            let no = h.latest.load(Ordering::Acquire);
            if no == 0 || no == self.last {
                return None;
            }
            let i = (no % h.slots as u64) as usize;
            let m = &h.metas[i];
            let s1 = m.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let format = Format::from_fourcc(m.fourcc.load(Ordering::Relaxed))?;
            let flags = m.flags.load(Ordering::Relaxed);
            let tc = m.timecode.load(Ordering::Relaxed);
            let frame = Frame {
                no: m.no.load(Ordering::Relaxed),
                info: FrameInfo {
                    width: m.width.load(Ordering::Relaxed),
                    height: m.height.load(Ordering::Relaxed),
                    stride: m.stride.load(Ordering::Relaxed),
                    format,
                    rate: (m.rate_num.load(Ordering::Relaxed), m.rate_den.load(Ordering::Relaxed)),
                    interlaced: flags & 1 != 0,
                    timecode: (flags & 2 != 0).then_some(Timecode {
                        hours: (tc >> 24) as u8,
                        minutes: (tc >> 16) as u8,
                        seconds: (tc >> 8) as u8,
                        frames: tc as u8,
                        drop_frame: flags & 4 != 0,
                        field_mark: flags & 8 != 0,
                    }),
                },
                captured_ns: m.captured_ns.load(Ordering::Relaxed),
            };
            let len = (m.len.load(Ordering::Relaxed) as usize).min(h.slot_bytes as usize);
            // SAFETY: inside the slot; a concurrent write is detected by `seq` below.
            let data = unsafe { std::slice::from_raw_parts(self.map.slot(i), len) };
            read(frame, data);
            fence(Ordering::Acquire);
            if m.seq.load(Ordering::Relaxed) != s1 || frame.no != no {
                continue;
            }
            self.last = no;
            if let Some(r) = self.slot.map(|k| &h.readers[k]) {
                r.seen.store(now_ms(), Ordering::Relaxed);
                r.frames.fetch_add(1, Ordering::Relaxed);
            }
            return Some(frame);
        }
        None
    }

    /// Attend jusqu'à `timeout` une image plus récente que la dernière lue (ou un changement d'état) :
    /// vrai s'il y en a une.
    pub fn wait(&self, timeout: Duration) -> bool {
        let h = self.map.header();
        let end = Instant::now() + timeout;
        loop {
            let w = h.wake.load(Ordering::Acquire);
            let no = h.latest.load(Ordering::Acquire);
            if no != 0 && no != self.last {
                return true;
            }
            if h.state.load(Ordering::Acquire) == STATE_CLOSED {
                return false;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            // Réveillé au plus tard toutes les 200 ms : un écrivain disparu ne réveille personne.
            futex_wait(&h.wake, w, left.min(Duration::from_millis(200)));
            if self.gone() {
                return false;
            }
        }
    }

    /// La prochaine image : attend jusqu'à `timeout`, puis la copie dans `buf`.
    pub fn next(&mut self, buf: &mut Vec<u8>, timeout: Duration) -> Next {
        if self.gone() {
            return Next::Gone;
        }
        let end = Instant::now() + timeout;
        loop {
            if let Some(f) = self.latest(buf) {
                return Next::Frame(f);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Next::Timeout;
            }
            if !self.wait(left) {
                return if self.gone() { Next::Gone } else { Next::Timeout };
            }
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let Some(k) = self.slot {
            let r = &self.map.header().readers[k];
            let _ = r.pid.compare_exchange(std::process::id(), 0, Ordering::AcqRel, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("strata-share-test-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    fn info(w: u32, h: u32) -> FrameInfo {
        FrameInfo { width: w, height: h, stride: w * 2, format: Format::Uyvy, rate: (25, 1), interlaced: true, timecode: None }
    }

    #[test]
    fn writes_and_reads() {
        let p = tmp("basic");
        let mut w = Writer::create(&p, 3, 64 * 32 * 2).unwrap();
        let mut r = Reader::open(&p, "livecut").unwrap();
        let mut buf = Vec::new();
        assert!(r.latest(&mut buf).is_none());
        assert_eq!(r.signal(), None);
        let tc = Timecode { hours: 10, minutes: 2, seconds: 3, frames: 24, drop_frame: false, field_mark: true };
        let data = vec![7u8; 64 * 32 * 2];
        w.publish(&FrameInfo { timecode: Some(tc), ..info(64, 32) }, &data).unwrap();
        let f = r.latest(&mut buf).unwrap();
        assert_eq!((f.no, f.info.width, f.info.height, f.info.timecode), (1, 64, 32, Some(tc)));
        assert!(f.info.interlaced);
        assert_eq!(buf, data);
        assert_eq!(r.signal(), Some(true));
        // Déjà lue.
        assert!(r.latest(&mut buf).is_none());
        // Trop grande.
        assert!(w.publish(&info(64, 64), &vec![0; 64 * 64 * 2 + PAGE]).is_err());
        // Le lecteur est inscrit avec son appli.
        let readers = w.readers();
        assert_eq!(readers.len(), 1);
        assert_eq!((readers[0].app.as_str(), readers[0].frames), ("livecut", 1));
        drop(r);
        assert!(w.readers().is_empty());
    }

    #[test]
    fn wakes_and_notices_the_end() {
        let p = tmp("wake");
        let mut w = Writer::create(&p, 3, 4096).unwrap();
        let mut r = Reader::open(&p, "multiview").unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut got = Vec::new();
            loop {
                match r.next(&mut buf, Duration::from_secs(5)) {
                    Next::Frame(f) => got.push((f.no, buf[0])),
                    Next::Timeout => panic!("no frame"),
                    Next::Gone => return got,
                }
            }
        });
        for i in 1..=5u8 {
            std::thread::sleep(Duration::from_millis(30));
            w.publish(&info(16, 8), &[i; 256]).unwrap();
        }
        std::thread::sleep(Duration::from_millis(30));
        drop(w);
        let got = t.join().unwrap();
        assert_eq!(got, (1..=5u64).map(|i| (i, i as u8)).collect::<Vec<_>>());
        assert!(!p.exists());
    }

    #[test]
    fn frames_are_never_torn() {
        let p = tmp("torn");
        let size = 256 * 1024;
        let mut w = Writer::create(&p, 2, size).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let readers: Vec<_> = (0..3)
            .map(|k| {
                let (p, stop) = (p.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut r = Reader::open(&p, &format!("app{k}")).unwrap();
                    let mut buf = Vec::new();
                    let mut n = 0;
                    while !stop.load(Ordering::Relaxed) {
                        if let Some(f) = r.latest(&mut buf) {
                            let b = (f.no % 251) as u8;
                            assert!(buf.iter().all(|&x| x == b), "a torn frame");
                            n += 1;
                        }
                    }
                    n
                })
            })
            .collect();
        let mut data = vec![0u8; size];
        for no in 1..=2000u64 {
            data.fill((no % 251) as u8);
            w.publish(&info(256, 512), &data).unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        for t in readers {
            assert!(t.join().unwrap() > 0);
        }
    }

    #[test]
    fn a_new_writer_replaces_the_file() {
        let p = tmp("replace");
        let w1 = Writer::create(&p, 2, 4096).unwrap();
        let r = Reader::open(&p, "cg").unwrap();
        assert!(!r.gone());
        // Un Core relancé recrée l'entrée : l'ancien lecteur le voit, l'ancien fichier reste lisible.
        let w2 = Writer::create(&p, 2, 4096).unwrap();
        assert!(r.gone());
        assert!(!Reader::observe(&p).unwrap().gone());
        // L'ancien écrivain ne retire pas le fichier du nouveau.
        drop(w1);
        assert!(p.exists());
        drop(w2);
        assert!(!p.exists());
    }

    #[test]
    fn names_and_limits() {
        std::env::remove_var("STRATA_SHARE_DIR");
        assert_eq!(sdi_path("7B10A04E", 3), PathBuf::from("/dev/shm/strata-sdi-7b10a04e-3"));
        assert_eq!(sdi_path("../x", 1).file_name().unwrap(), "strata-sdi-___x-1");
        let p = tmp("full");
        let _w = Writer::create(&p, 2, 4096).unwrap();
        let rs: Vec<_> = (0..MAX_READERS).map(|i| Reader::open(&p, &"é".repeat(30 + i)).unwrap()).collect();
        assert!(Reader::open(&p, "one-too-many").is_err());
        assert_eq!(rs[0].readers()[0].app, "é".repeat(19));
        assert!(Writer::create(&p, 1, 4096).is_err());
        let junk = tmp("junk");
        fs::write(&junk, vec![1u8; 8192]).unwrap();
        assert!(Reader::observe(&junk).is_err());
    }
}
