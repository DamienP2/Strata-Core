//! L'aperçu d'un flux ST 2110-20 reçu : le Core rejoint le groupe multicast seulement pendant
//! qu'on regarde le flux dans les réglages, et le quitte ensuite.
//!
//! - Une image par seconde : le fil lit les paquets le temps d'une image entière, puis ne lit plus
//!   rien jusqu'à la seconde suivante (le noyau jette ce qui arrive entre-temps). Un flux UHD
//!   (12 Gb/s) ne peut pas être lu en entier par un programme ordinaire ; une image sur cinquante,
//!   si.
//! - Une toile qui garde chaque ligne reçue : un paquet perdu laisse la ligne de l'image d'avant.
//! - RFC 4175 : YCbCr 4:2:2, 10 bits (le format courant) ou 8 bits (reconnu à la largeur des
//!   lignes) ; progressif ou entrelacé (le bit F des lignes) ; cadence d'après l'horodatage RTP.
//! - SMPTE 2022-7 : le réseau rouge d'abord ; sans paquet pendant 2 s, le bleu, et ainsi de suite.
use serde::Serialize;
use std::{
    io,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const MAX_W: usize = 4096;
const MAX_H: usize = 2160;
const STRIDE: usize = MAX_W * 2;

/// Une patte du flux : son adresse, l'adresse de la carte réseau qui la reçoit, « red » ou « blue ».
#[derive(Clone, Debug)]
pub struct Leg {
    pub group: SocketAddrV4,
    pub iface: Ipv4Addr,
    pub name: &'static str,
}

/// Où en est la réception.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Des paquets arrivent.
    pub receiving: bool,
    /// La patte écoutée (« red », « blue »).
    pub leg: Option<&'static str>,
    /// « 1080i50 ».
    pub format: Option<String>,
    pub error: Option<String>,
}

/// La dernière image assemblée : UYVY 8 bits, `width × height`, `width * 2` octets par ligne.
#[derive(Clone)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub data: Arc<Vec<u8>>,
}

// ---------- RFC 4175 ----------

/// Les lignes reçues, gardées d'une image à l'autre.
pub struct Canvas {
    /// Une toile par trame (la seconde ne sert qu'en entrelacé).
    fields: [Vec<u8>; 2],
    /// La largeur vue, en pixels, selon la profondeur supposée (10 bits, 8 bits).
    seen_w: [u32; 2],
    lines: [u32; 2],
    eight_bits: bool,
    interlaced: bool,
    /// L'horodatage RTP de la dernière fin d'image, et l'écart entre deux (90 kHz).
    last_ts: Option<u32>,
    period: Option<u32>,
    /// Un écart plus long vu une fois.
    longer: Option<u32>,
    /// Fins d'image (bit marqueur) vues.
    pub markers: u64,
    pub packets: u64,
}

impl Default for Canvas {
    fn default() -> Self {
        Canvas {
            fields: [vec![0u8; STRIDE * MAX_H], Vec::new()],
            seen_w: [0; 2],
            lines: [0; 2],
            eight_bits: false,
            interlaced: false,
            last_ts: None,
            period: None,
            longer: None,
            markers: 0,
            packets: 0,
        }
    }
}

const WIDTHS: [u32; 7] = [720, 1280, 1440, 1920, 2048, 3840, 4096];

impl Canvas {
    /// Un paquet RTP ; vrai s'il finit une image (ou une trame).
    pub fn packet(&mut self, p: &[u8]) -> bool {
        if p.len() < 12 || p[0] >> 6 != 2 {
            return false;
        }
        let marker = p[1] & 0x80 != 0;
        let ts = u32::from_be_bytes([p[4], p[5], p[6], p[7]]);
        let mut at = 12 + 4 * (p[0] & 0x0f) as usize;
        if p[0] & 0x10 != 0 {
            if p.len() < at + 4 {
                return false;
            }
            at += 4 + 4 * u16::from_be_bytes([p[at + 2], p[at + 3]]) as usize;
        }
        let mut end = p.len();
        if p[0] & 0x20 != 0 {
            end = end.saturating_sub(*p.last().unwrap_or(&0) as usize);
        }
        // Le numéro de séquence étendu, puis les en-têtes de lignes (6 octets chacun).
        at += 2;
        let mut heads = Vec::with_capacity(4);
        loop {
            if at + 6 > end {
                return false;
            }
            let len = u16::from_be_bytes([p[at], p[at + 1]]) as usize;
            let field = (p[at + 2] >> 7) as usize;
            let line = (u16::from_be_bytes([p[at + 2], p[at + 3]]) & 0x7fff) as usize;
            let more = p[at + 4] & 0x80 != 0;
            let offset = (u16::from_be_bytes([p[at + 4], p[at + 5]]) & 0x7fff) as usize;
            heads.push((len, field, line, offset));
            at += 6;
            if !more {
                break;
            }
        }
        self.packets += 1;
        for (len, field, line, offset) in heads {
            if at + len > end {
                break;
            }
            self.line(&p[at..at + len], field, line, offset);
            at += len;
        }
        if marker {
            self.markers += 1;
            if let Some(last) = self.last_ts {
                let d = ts.wrapping_sub(last);
                // Une image perdue double l'écart : un écart plus court est pris aussitôt, un plus
                // long seulement s'il revient (la source a vraiment changé de cadence).
                if d > 0 && d < 90_000 {
                    match self.period {
                        Some(p) if d > p && self.longer != Some(d) => self.longer = Some(d),
                        _ => {
                            self.period = Some(d);
                            self.longer = None;
                        }
                    }
                }
            }
            self.last_ts = Some(ts);
            // 8 bits : la largeur vue n'est une largeur courante qu'en lisant 4 octets par 2 pixels.
            let w = |px: u32| WIDTHS.contains(&px);
            if self.seen_w[0] > 0 {
                self.eight_bits = w(self.seen_w[1]) && !w(self.seen_w[0]);
            }
        }
        marker
    }

    fn line(&mut self, data: &[u8], field: usize, line: usize, offset: usize) {
        if field == 1 && !self.interlaced {
            self.interlaced = true;
            self.fields[1] = vec![0u8; STRIDE * MAX_H / 2];
        }
        let rows = if field == 1 { MAX_H / 2 } else { MAX_H };
        if line >= rows {
            return;
        }
        // Largeur vue, selon les deux profondeurs possibles.
        self.seen_w[0] = self.seen_w[0].max((offset + data.len() / 5 * 2) as u32);
        self.seen_w[1] = self.seen_w[1].max((offset + data.len() / 4 * 2) as u32);
        self.lines[field] = self.lines[field].max(line as u32 + 1);
        let row = &mut self.fields[field][line * STRIDE..(line + 1) * STRIDE];
        let mut x = offset * 2;
        // Un paquet au-delà de la ligne (malformé, ou un autre émetteur plus large) : ignoré.
        if x >= STRIDE {
            return;
        }
        if self.eight_bits {
            let n = data.len().min(STRIDE.saturating_sub(x));
            row[x..x + n].copy_from_slice(&data[..n]);
            return;
        }
        // 10 bits : Cb Y0 Cr Y1 sur 5 octets ; on garde les 8 bits forts de chacun.
        for g in data.as_chunks::<5>().0 {
            if x + 4 > STRIDE {
                break;
            }
            row[x] = g[0];
            row[x + 1] = ((g[1] & 0x3f) << 2) | (g[2] >> 6);
            row[x + 2] = ((g[2] & 0x0f) << 4) | (g[3] >> 4);
            row[x + 3] = ((g[3] & 0x03) << 6) | (g[4] >> 2);
            x += 4;
        }
    }

    fn width(&self) -> u32 {
        (self.seen_w[self.eight_bits as usize] as usize).min(MAX_W) as u32 & !1
    }

    /// « 1080i50 », « 2160p59.94 ».
    pub fn format(&self) -> Option<String> {
        let h = if self.interlaced { self.lines[0].max(self.lines[1]) * 2 } else { self.lines[0] };
        if h == 0 {
            return None;
        }
        let rate = self.period.map(|d| 90_000.0 / d as f64);
        let rate = rate.map(|r| {
            let r = (r * 100.0).round() / 100.0;
            if r.fract() == 0.0 {
                format!("{r:.0}")
            } else {
                format!("{r}")
            }
        });
        Some(format!("{h}{}{}", if self.interlaced { 'i' } else { 'p' }, rate.unwrap_or_default()))
    }

    /// L'image, trames entrelacées remises ensemble.
    pub fn picture(&self) -> Option<Picture> {
        let w = self.width() as usize;
        if w < 16 || self.lines[0] < 2 {
            return None;
        }
        let h = if self.interlaced { (self.lines[0].max(self.lines[1]) * 2) as usize } else { self.lines[0] as usize }.min(MAX_H);
        let mut out = Vec::with_capacity(w * 2 * h);
        for y in 0..h {
            let (f, l) = if self.interlaced { (y % 2, y / 2) } else { (0, y) };
            out.extend_from_slice(&self.fields[f][l * STRIDE..l * STRIDE + w * 2]);
        }
        Some(Picture { width: w as u32, height: h as u32, data: Arc::new(out) })
    }
}

// ---------- La réception ----------

/// Une socket UDP qui rejoint le groupe (ou écoute une adresse simple) ; partagée avec les
/// autres programmes qui reçoivent le même flux (SO_REUSEADDR).
fn open(leg: &Leg) -> io::Result<UdpSocket> {
    use std::os::fd::FromRawFd;
    // SAFETY: plain socket calls on a descriptor we own; it is handed to UdpSocket right after.
    let sock = unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let s = UdpSocket::from_raw_fd(fd);
        let one: libc::c_int = 1;
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, (&one as *const libc::c_int).cast(), 4);
        // Une image HD tient dans 8 Mo ; le système peut en limiter la taille (net.core.rmem_max).
        let buf: libc::c_int = 32 << 20;
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVBUF, (&buf as *const libc::c_int).cast(), 4);
        let addr = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: leg.group.port().to_be(),
            sin_addr: libc::in_addr { s_addr: u32::from(*leg.group.ip()).to_be() },
            sin_zero: [0; 8],
        };
        if libc::bind(fd, (&addr as *const libc::sockaddr_in).cast(), std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t) != 0 {
            return Err(io::Error::last_os_error());
        }
        s
    };
    if leg.group.ip().is_multicast() {
        sock.join_multicast_v4(leg.group.ip(), &leg.iface)?;
    }
    sock.set_read_timeout(Some(Duration::from_millis(100)))?;
    Ok(sock)
}

/// La réception d'un flux, tant qu'on la garde (arrêtée, la socket quitte le groupe).
pub struct Receiver {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Mutex<(Status, Option<Picture>)>>,
}

impl Receiver {
    pub fn start(legs: Vec<Leg>) -> Receiver {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new((Status::default(), None)));
        let thread = {
            let (stop, shared) = (stop.clone(), shared.clone());
            std::thread::Builder::new().name("st2110-preview".into()).spawn(move || run(&legs, &stop, &shared)).ok()
        };
        Receiver { stop, thread, shared }
    }

    pub fn status(&self) -> Status {
        self.shared.lock().unwrap().0.clone()
    }

    pub fn picture(&self) -> Option<Picture> {
        self.shared.lock().unwrap().1.clone()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(legs: &[Leg], stop: &AtomicBool, shared: &Mutex<(Status, Option<Picture>)>) {
    if legs.is_empty() {
        return;
    }
    let mut canvas = Canvas::default();
    let mut buf = vec![0u8; 9000];
    let mut k = 0;
    while !stop.load(Ordering::Relaxed) {
        let leg = &legs[k % legs.len()];
        tracing::debug!("previewing ST 2110 flow {} ({})", leg.group, leg.name);
        shared.lock().unwrap().0 = Status { leg: Some(leg.name), ..Status::default() };
        let mut last_packet = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let window = Instant::now();
            // Le groupe rejoint le temps d'une image seulement, puis quitté (socket fermé) : entre
            // deux prises, le serveur ne reçoit pas le flux (2,6 Gb/s en 1080p50, 10 en 2160p50)
            // pour le jeter ; et rien n'attend dans la file à la prise suivante.
            let sock = match open(leg) {
                Ok(s) => s,
                Err(e) => {
                    shared.lock().unwrap().0 = Status { leg: Some(leg.name), error: Some(format!("cannot join {}: {e}", leg.group)), ..Status::default() };
                    if sleep(stop, Duration::from_secs(2)) {
                        return;
                    }
                    break;
                }
            };
            // Une image entière : de la fin d'une image à la fin de la suivante (en entrelacé, deux
            // trames), 400 ms au plus.
            let need = if canvas.interlaced { 3 } else { 2 };
            let mut ends = 0;
            let mut got = 0u64;
            while ends < need && window.elapsed() < Duration::from_millis(400) && !stop.load(Ordering::Relaxed) {
                match sock.recv(&mut buf) {
                    Ok(n) => {
                        got += 1;
                        if canvas.packet(&buf[..n]) {
                            ends += 1;
                        }
                    }
                    Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                    Err(_) => break,
                }
            }
            drop(sock);
            if got > 0 {
                last_packet = Instant::now();
            }
            {
                let mut g = shared.lock().unwrap();
                g.0.receiving = got > 0;
                g.0.format = canvas.format();
                g.0.error = None;
                if got > 0 {
                    if let Some(p) = canvas.picture() {
                        g.1 = Some(p);
                    }
                }
            }
            // Rien depuis 2 s : l'autre réseau, s'il y en a un.
            if legs.len() > 1 && last_packet.elapsed() > Duration::from_secs(2) {
                break;
            }
            if sleep(stop, Duration::from_secs(1).saturating_sub(window.elapsed())) {
                return;
            }
        }
        k += 1;
    }
}

fn sleep(stop: &AtomicBool, d: Duration) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50).min(end.saturating_duration_since(Instant::now())));
    }
    stop.load(Ordering::Relaxed)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Les paquets RFC 4175 d'une image `w × h` (10 bits, ou 8), `per` pixels par paquet ; en
    /// entrelacé, deux trames. Chaque pixel vaut Y = `y`, Cb = Cr = 128.
    pub fn packets(w: u32, h: u32, interlaced: bool, ten: bool, y: u8, ts: u32, per: u32) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let fields = if interlaced { 2 } else { 1 };
        let rows = if interlaced { h / 2 } else { h };
        for f in 0..fields {
            for line in 0..rows {
                let mut x = 0;
                while x < w {
                    let n = per.min(w - x);
                    let mut p = vec![0x80, 96, 0, 0];
                    // En entrelacé, chaque trame a son horodatage (une demi-image plus tard).
                    p.extend_from_slice(&(ts + f * 1800).to_be_bytes());
                    p.extend_from_slice(&[0, 0, 0, 1, 0, 0]);
                    let bytes = if ten { n / 2 * 5 } else { n * 2 };
                    p.extend_from_slice(&(bytes as u16).to_be_bytes());
                    p.extend_from_slice(&((line as u16) | ((f as u16) << 15)).to_be_bytes());
                    p.extend_from_slice(&(x as u16).to_be_bytes());
                    for _ in 0..n / 2 {
                        if ten {
                            // Cb=512 Y0=y<<2 Cr=512 Y1=y<<2, sur 10 bits chacun.
                            let (cb, yy, cr) = (512u64, (y as u64) << 2, 512u64);
                            let v = (cb << 30) | (yy << 20) | (cr << 10) | yy;
                            p.extend_from_slice(&v.to_be_bytes()[3..]);
                        } else {
                            p.extend_from_slice(&[128, y, 128, y]);
                        }
                    }
                    x += n;
                    if x >= w && line == rows - 1 {
                        p[1] |= 0x80;
                    }
                    out.push(p);
                }
            }
        }
        out
    }

    #[test]
    fn assembles_ten_and_eight_bit_pictures() {
        for (ten, interlaced) in [(true, false), (true, true), (false, false)] {
            let mut c = Canvas::default();
            for ts in [0u32, 3600, 7200] {
                for p in packets(1920, 1080, interlaced, ten, 180, ts, 480) {
                    c.packet(&p);
                }
            }
            let pic = c.picture().unwrap();
            assert_eq!((pic.width, pic.height), (1920, 1080), "ten={ten} interlaced={interlaced}");
            assert_eq!(&pic.data[..4], &[128, 180, 128, 180]);
            let last = pic.data.len() - 4;
            assert_eq!(&pic.data[last..], &[128, 180, 128, 180]);
            // 3600 à 90 kHz : 25 images par seconde ; en entrelacé, 50 trames.
            assert_eq!(c.format().unwrap(), if interlaced { "1080i50" } else { "1080p25" });
        }
    }

    #[test]
    fn receives_over_udp() {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = sock.local_addr().unwrap().port();
        drop(sock);
        let rx = Receiver::start(vec![Leg { group: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port), iface: Ipv4Addr::UNSPECIFIED, name: "red" }]);
        let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        let mut ts = 0u32;
        // Jusqu'à une image et une cadence établie (une machine chargée perd des paquets au début).
        while (rx.picture().is_none() || rx.status().format.as_deref() != Some("720p50")) && Instant::now() < end {
            for p in packets(1280, 720, false, true, 90, ts, 640) {
                let _ = tx.send_to(&p, ("127.0.0.1", port));
            }
            ts = ts.wrapping_add(1800);
            std::thread::sleep(Duration::from_millis(20));
        }
        let pic = rx.picture().expect("a picture");
        assert_eq!((pic.width, pic.height), (1280, 720));
        let st = rx.status();
        assert_eq!(st.leg, Some("red"));
        assert_eq!(st.format.as_deref(), Some("720p50"));
    }
}
