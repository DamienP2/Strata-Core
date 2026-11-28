//! IPMX (VSF TR-10) : les rapports RTCP des émetteurs.
//!
//! Chaque flux envoyé en IPMX donne, sur le port média + 1 et vers la même adresse, un RTCP
//! Sender Report (RFC 3550 § 6.4.1) suivi du bloc d'information IPMX (TR-10-1 § 8.7) : l'horloge
//! de référence (`ts-refclk`), celle du média (`mediaclk`), puis la description du média
//! (TR-10-2 pour la vidéo non compressée, TR-10-3 pour le son PCM). Le rapport part avant le
//! premier paquet de l'image (vidéo : une fois par image, par trame en entrelacé) ou toutes les
//! 10 ms (son). L'horodatage « NTP » est l'heure interne (TAI) en secondes et nanosecondes, celle
//! qui correspond à l'horodatage RTP du rapport.
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

/// Le marqueur du bloc IPMX (« X1 »).
const IPMX_TAG: u16 = 0x5831;

/// La description d'une vidéo non compressée (TR-10-2, bloc de type 1).
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    /// « YCbCr-4:2:2 ».
    pub sampling: String,
    pub depth: u8,
    /// General Packing Mode (PM=2110GPM) ; sinon Block.
    pub gpm: bool,
    pub interlaced: bool,
    pub segmented: bool,
    pub par: (u8, u8),
    pub range: String,
    pub colorimetry: String,
    pub tcs: String,
    pub width: u16,
    pub height: u16,
    pub rate: (u32, u32),
    pub pixel_clock: u64,
    pub htotal: u16,
    pub vtotal: u16,
}

/// La description d'un son PCM (TR-10-3, bloc de type 2).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub bits: u8,
    pub channels: u8,
    pub ptime_us: u16,
    pub measured_rate: u32,
    /// « SMPTE2110.(ST) ».
    pub channel_order: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MediaInfo {
    Video(VideoInfo),
    Audio(AudioInfo),
    /// Sans bloc de média (données annexes, JPEG XS : aucun bloc défini par TR-10-1/2/3).
    None,
}

/// Une chaîne de taille fixe, complétée par des zéros.
fn fixed(out: &mut Vec<u8>, s: &str, n: usize) {
    let b = s.as_bytes();
    let len = b.len().min(n);
    out.extend_from_slice(&b[..len]);
    out.resize(out.len() + n - len, 0);
}

fn u16be(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn u32be(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Le bloc d'information IPMX (TR-10-1 figure 1) : `version` change quand le média change.
pub fn info_block(refclk: &str, mediaclk: &str, version: u8, media: &MediaInfo) -> Vec<u8> {
    let mut b = Vec::with_capacity(176);
    u16be(&mut b, IPMX_TAG);
    u16be(&mut b, 0); // la longueur, remplie à la fin
    b.push(version);
    b.extend_from_slice(&[0, 0, 0]);
    fixed(&mut b, refclk, 64);
    fixed(&mut b, mediaclk, 12);
    match media {
        MediaInfo::Video(v) => {
            let at = b.len();
            u16be(&mut b, 0x0001);
            u16be(&mut b, 0);
            fixed(&mut b, &v.sampling, 16);
            b.push(v.depth & 0x7f);
            b.push((v.gpm as u8) << 7 | (v.interlaced as u8) << 6 | (v.segmented as u8) << 5);
            b.push(v.par.0);
            b.push(v.par.1);
            fixed(&mut b, &v.range, 12);
            fixed(&mut b, &v.colorimetry, 20);
            fixed(&mut b, &v.tcs, 16);
            u16be(&mut b, v.width);
            u16be(&mut b, v.height);
            u32be(&mut b, (v.rate.0 & 0x3f_ffff) << 10 | (v.rate.1 & 0x3ff));
            b.extend_from_slice(&v.pixel_clock.to_be_bytes());
            u16be(&mut b, v.htotal);
            u16be(&mut b, v.vtotal);
            let words = ((b.len() - at) / 4 - 1) as u16;
            b[at + 2..at + 4].copy_from_slice(&words.to_be_bytes());
        }
        MediaInfo::Audio(a) => {
            let at = b.len();
            u16be(&mut b, 0x0002);
            u16be(&mut b, 0);
            u32be(&mut b, a.sample_rate);
            b.push(a.bits);
            b.push(a.channels);
            u16be(&mut b, a.ptime_us);
            u32be(&mut b, a.measured_rate);
            let order = a.channel_order.len().div_ceil(4) * 4;
            u32be(&mut b, (order / 4) as u32);
            fixed(&mut b, &a.channel_order, order);
            let words = ((b.len() - at) / 4 - 1) as u16;
            b[at + 2..at + 4].copy_from_slice(&words.to_be_bytes());
        }
        MediaInfo::None => {}
    }
    let words = (b.len() / 4 - 1) as u16;
    b[2..4].copy_from_slice(&words.to_be_bytes());
    b
}

/// Un Sender Report RTCP avec le bloc IPMX (TR-10-1 figure 1).
pub fn sender_report(ssrc: u32, tai_ns: u64, rtp: u32, packets: u32, octets: u32, block: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(28 + block.len());
    p.push(0x80); // V=2, P=0, RC=0
    p.push(200); // SR
    u16be(&mut p, ((28 + block.len()) / 4 - 1) as u16);
    u32be(&mut p, ssrc);
    // L'heure interne au format PTP tronqué : secondes, puis nanosecondes.
    u32be(&mut p, (tai_ns / 1_000_000_000) as u32);
    u32be(&mut p, (tai_ns % 1_000_000_000) as u32);
    u32be(&mut p, rtp);
    u32be(&mut p, packets);
    u32be(&mut p, octets);
    p.extend_from_slice(block);
    p
}

/// L'horodatage RTP d'un instant TAI pour une horloge de `rate` Hz (ST 2110-10).
pub fn rtp_of(tai_ns: u64, rate: u64) -> u32 {
    ((tai_ns as u128 * rate as u128 / 1_000_000_000) & 0xffff_ffff) as u32
}

/// Les totaux d'une image vidéo : (htotal, vtotal, horloge pixel en Hz), d'après les formats
/// SMPTE (ST 274, 296, 2036) ; pour une image qui ne vient pas d'un signal en bande de base, ce
/// sont les valeurs du format.
pub fn video_totals(width: u32, height: u32, (n, d): (u32, u32), interlaced: bool) -> (u16, u16, u64) {
    // Images par seconde (une image : deux trames en entrelacé).
    let fps = n as f64 / d.max(1) as f64;
    let fam = |f: f64| {
        if (f - 24.0).abs() < 0.1 || (f - 23.976).abs() < 0.1 {
            24
        } else if (f % 25.0).abs() < 0.01 {
            25
        } else {
            30
        }
    };
    let family = fam(if interlaced { fps * 2.0 } else { fps }.min(fps * 2.0));
    let (htotal, vtotal) = match (height, family) {
        (2160, 24) => (5500, 2250),
        (2160, 25) => (5280, 2250),
        (2160, _) => (4400, 2250),
        (1080, 24) => (2750, 1125),
        (1080, 25) => (2640, 1125),
        (1080, _) => (2200, 1125),
        (720, 24) => (4125, 750),
        (720, 25) => (1980 * if fps < 40.0 { 2 } else { 1 }, 750),
        (720, _) => (1650 * if fps < 40.0 { 2 } else { 1 }, 750),
        (576, _) => (864, 625),
        (486, _) => (858, 525),
        _ => (width + width / 6, height + height / 24),
    };
    let pixel_clock = (htotal as f64 * vtotal as f64 * fps).round() as u64;
    (htotal as u16, vtotal as u16, pixel_clock)
}

/// Les rapports d'un flux envoyé : un socket par patte (rouge, bleue), vers l'adresse du flux,
/// port + 1, depuis l'interface du port média.
pub struct Reporter {
    legs: Vec<(UdpSocket, SocketAddrV4)>,
    pub ssrc: u32,
    block: Vec<u8>,
    packets: u32,
    octets: u32,
}

impl Reporter {
    /// `legs` : (adresse de l'interface du port média, adresse et port du flux).
    pub fn open(legs: &[(Ipv4Addr, SocketAddrV4)], ssrc: u32, block: Vec<u8>) -> Result<Reporter, String> {
        let mut out = Vec::new();
        for (iface, dst) in legs {
            let sock = UdpSocket::bind((*iface, 0)).map_err(|e| format!("RTCP: cannot use {iface}: {e}"))?;
            if dst.ip().is_multicast() {
                let _ = sock.set_multicast_ttl_v4(64);
                set_multicast_if(&sock, *iface);
            }
            out.push((sock, SocketAddrV4::new(*dst.ip(), dst.port().wrapping_add(1))));
        }
        Ok(Reporter { legs: out, ssrc, block, packets: 0, octets: 0 })
    }

    /// Le média a changé : un nouveau bloc (sa version suit).
    pub fn set_block(&mut self, block: Vec<u8>) {
        self.block = block;
    }

    /// Envoie le rapport de l'image qui part (avant ses paquets), puis compte ses paquets.
    pub fn report(&mut self, tai_ns: u64, rtp: u32, packets: u32, octets: u32) {
        let p = sender_report(self.ssrc, tai_ns, rtp, self.packets, self.octets, &self.block);
        for (s, dst) in &self.legs {
            let _ = s.send_to(&p, dst);
        }
        self.packets = self.packets.wrapping_add(packets);
        self.octets = self.octets.wrapping_add(octets);
    }
}

fn set_multicast_if(sock: &UdpSocket, iface: Ipv4Addr) {
    use std::os::fd::AsRawFd;
    let addr = libc::in_addr { s_addr: u32::from_ne_bytes(iface.octets()) };
    // SAFETY: setsockopt on a socket we own, with an in_addr of the right size.
    unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_MULTICAST_IF,
            &addr as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::in_addr>() as libc::socklen_t,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> VideoInfo {
        VideoInfo {
            sampling: "YCbCr-4:2:2".into(),
            depth: 10,
            gpm: true,
            interlaced: false,
            segmented: false,
            par: (1, 1),
            range: "NARROW".into(),
            colorimetry: "BT709".into(),
            tcs: "SDR".into(),
            width: 1920,
            height: 1080,
            rate: (60000, 1001),
            pixel_clock: 148_550_104,
            htotal: 2200,
            vtotal: 1125,
        }
    }

    /// L'exemple de TR-10-2 § 11 : 204 octets, longueur 50 ; bloc IPMX 43, bloc vidéo 22.
    #[test]
    fn video_report_matches_tr_10_2() {
        let block = info_block("localmac=00-20-FC-32-2F-40", "sender", 0, &MediaInfo::Video(example()));
        let p = sender_report(3254, 1_665_165_600 * 1_000_000_000 + 262_167_158, 610_164_507, 0, 0, &block);
        assert_eq!(p.len(), 204);
        assert_eq!((p[0], p[1], u16::from_be_bytes([p[2], p[3]])), (0x80, 200, 50));
        assert_eq!(u32::from_be_bytes(p[4..8].try_into().unwrap()), 3254);
        assert_eq!(u32::from_be_bytes(p[8..12].try_into().unwrap()), 1_665_165_600);
        assert_eq!(u32::from_be_bytes(p[12..16].try_into().unwrap()), 262_167_158);
        assert_eq!(u32::from_be_bytes(p[16..20].try_into().unwrap()), 610_164_507);
        // Le bloc IPMX : « X1 », 43 mots moins un… soit 44 mots.
        assert_eq!(&p[28..30], &[0x58, 0x31]);
        assert_eq!(u16::from_be_bytes([p[30], p[31]]), 43);
        assert_eq!(&p[36..36 + 26], b"localmac=00-20-FC-32-2F-40");
        assert_eq!(&p[100..106], b"sender");
        // Le bloc vidéo : type 1, 22.
        let m = 112;
        assert_eq!(u16::from_be_bytes([p[m], p[m + 1]]), 1);
        assert_eq!(u16::from_be_bytes([p[m + 2], p[m + 3]]), 22);
        assert_eq!(&p[m + 4..m + 15], b"YCbCr-4:2:2");
        assert_eq!((p[m + 20], p[m + 21], p[m + 22], p[m + 23]), (10, 0x80, 1, 1));
        let r = u32::from_be_bytes(p[m + 76..m + 80].try_into().unwrap());
        assert_eq!((r >> 10, r & 0x3ff), (60000, 1001));
        assert_eq!(u64::from_be_bytes(p[m + 80..m + 88].try_into().unwrap()), 148_550_104);
        assert_eq!((u16::from_be_bytes([p[m + 88], p[m + 89]]), u16::from_be_bytes([p[m + 90], p[m + 91]])), (2200, 1125));
    }

    #[test]
    fn audio_block() {
        let a = AudioInfo { sample_rate: 48000, bits: 24, channels: 8, ptime_us: 125, measured_rate: 47952, channel_order: "SMPTE2110.(U08)".into() };
        let b = info_block("ptp=IEEE1588-2008:traceable", "direct=0", 3, &MediaInfo::Audio(a));
        // 84 + (4 + 4 + 4 + 4 + 4 + 16) = 120 octets.
        assert_eq!(b.len(), 120);
        assert_eq!(u16::from_be_bytes([b[2], b[3]]), 29);
        assert_eq!(b[4], 3);
        assert_eq!(u16::from_be_bytes([b[84], b[85]]), 2);
        assert_eq!(u16::from_be_bytes([b[86], b[87]]), 8);
        assert_eq!(u32::from_be_bytes(b[88..92].try_into().unwrap()), 48000);
        assert_eq!((b[92], b[93], u16::from_be_bytes([b[94], b[95]])), (24, 8, 125));
        assert_eq!(u32::from_be_bytes(b[100..104].try_into().unwrap()), 4);
        assert_eq!(&b[104..119], b"SMPTE2110.(U08)");
    }

    #[test]
    fn totals_of_usual_formats() {
        assert_eq!(video_totals(1920, 1080, (50, 1), false), (2640, 1125, 148_500_000));
        assert_eq!(video_totals(1920, 1080, (25, 1), true), (2640, 1125, 74_250_000));
        assert_eq!(video_totals(1920, 1080, (60000, 1001), false), (2200, 1125, 148_351_648));
        assert_eq!(video_totals(1280, 720, (50, 1), false), (1980, 750, 74_250_000));
        assert_eq!(video_totals(3840, 2160, (50, 1), false), (5280, 2250, 594_000_000));
        assert_eq!(rtp_of(1_000_000_000, 90000), 90000);
    }
}
