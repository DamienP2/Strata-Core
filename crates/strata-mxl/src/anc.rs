//! Données annexes (ANC, SMPTE ST 291) : la charge RFC 8331 des grains MXL `video/smpte291` et
//! des paquets ST 2110-40. Un grain garde les octets à partir du champ *Length* (les 14 premiers,
//! en-tête RTP et numéro de séquence étendu, n'y sont pas).
//!
//! ```text
//! Length(16) ANC_Count(8) F(2) reserved(22)
//! puis par paquet : C(1) Line(11) Offset(12) S(1) StreamNum(7)
//!                   DID(10) SDID(10) Data_Count(10) UDW(10 × n) Checksum(10), aligné sur 32 bits
//! ```

/// Un paquet ANC. Les mots sont donnés sur 8 bits : la parité (bits 8 et 9) et la somme de
/// contrôle sont calculées à l'écriture.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Packet {
    /// Dans la chrominance (sinon la luminance).
    pub c: bool,
    /// La ligne (0x7FF : sans ligne précise ; 0x7FE : n'importe quelle ligne de l'effacement).
    pub line: u16,
    /// La position horizontale (0xFFF : sans position).
    pub offset: u16,
    /// Plusieurs flux de données (S), et le numéro de celui-ci.
    pub s: bool,
    pub stream: u8,
    pub did: u8,
    pub sdid: u8,
    /// Les mots de données (255 au plus).
    pub udw: Vec<u8>,
}

/// Le champ F : 0 progressif, 2 première trame, 3 seconde trame.
pub type Field = u8;

/// Les 8 bits d'un mot avec ses bits de parité (b8 : parité paire de b0-b7, b9 = !b8).
pub fn with_parity(v: u8) -> u16 {
    let p = (v.count_ones() & 1) as u16;
    v as u16 | (p << 8) | ((p ^ 1) << 9)
}

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, bits: u32) {
        self.acc = (self.acc << bits) | (v as u64 & ((1u64 << bits) - 1));
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
    }
    /// Jusqu'à la prochaine frontière de 32 bits (depuis `start`, en octets).
    fn align32(&mut self, start: usize) {
        if self.n > 0 {
            self.put(0, 8 - self.n);
        }
        while !(self.out.len() - start).is_multiple_of(4) {
            self.out.push(0);
        }
    }
}

/// La charge d'un grain (à partir de Length) pour ces paquets. Au plus 255 paquets ; les mots en
/// trop d'un paquet (plus de 255) sont coupés.
pub fn encode(packets: &[Packet], field: Field) -> Vec<u8> {
    let mut b = Bits { out: Vec::with_capacity(64), acc: 0, n: 0 };
    b.put(0, 16); // Length, posée à la fin
    b.put(packets.len().min(255) as u32, 8);
    b.put(field as u32 & 3, 2);
    b.put(0, 22);
    let start = b.out.len();
    for p in packets.iter().take(255) {
        let pos = b.out.len();
        b.put(p.c as u32, 1);
        b.put(p.line as u32, 11);
        b.put(p.offset as u32, 12);
        b.put(p.s as u32, 1);
        b.put(p.stream as u32, 7);
        let udw = &p.udw[..p.udw.len().min(255)];
        let words = [with_parity(p.did), with_parity(p.sdid), with_parity(udw.len() as u8)];
        let mut sum: u32 = 0;
        for w in words.iter().copied().chain(udw.iter().map(|&v| with_parity(v))) {
            b.put(w as u32, 10);
            sum += w as u32 & 0x1FF;
        }
        let cs = sum & 0x1FF;
        b.put(cs | ((!cs >> 8) & 1) << 9, 10);
        b.align32(pos);
    }
    let len = (b.out.len() - start) as u16;
    b.out[..2].copy_from_slice(&len.to_be_bytes());
    b.out
}

struct Read<'a> {
    data: &'a [u8],
    bit: usize,
}

impl Read<'_> {
    fn get(&mut self, bits: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..bits {
            let byte = *self.data.get(self.bit / 8)?;
            v = (v << 1) | ((byte >> (7 - self.bit % 8)) & 1) as u32;
            self.bit += 1;
        }
        Some(v)
    }
}

/// Les paquets d'un grain (et son champ F). None si la charge est mal formée.
pub fn decode(data: &[u8]) -> Option<(Vec<Packet>, Field)> {
    let mut r = Read { data, bit: 0 };
    let len = r.get(16)? as usize;
    let count = r.get(8)? as usize;
    let field = r.get(2)? as Field;
    r.get(22)?;
    // Length compte à partir du premier paquet, après les 6 octets d'en-tête.
    let end = 6 + len;
    if end > data.len() {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let pos = r.bit;
        let c = r.get(1)? == 1;
        let line = r.get(11)? as u16;
        let offset = r.get(12)? as u16;
        let s = r.get(1)? == 1;
        let stream = r.get(7)? as u8;
        let did = r.get(10)? as u8;
        let sdid = r.get(10)? as u8;
        let n = r.get(10)? as u8 as usize;
        let mut udw = Vec::with_capacity(n);
        for _ in 0..n {
            udw.push(r.get(10)? as u8);
        }
        r.get(10)?; // somme de contrôle
                    // Aligné sur 32 bits depuis le début du paquet.
        let used = r.bit - pos;
        r.bit += (32 - used % 32) % 32;
        if r.bit > end * 8 {
            return None;
        }
        out.push(Packet { c, line, offset, s, stream, did, sdid, udw });
    }
    Some((out, field))
}

/// Le timecode d'un paquet ATC (SMPTE ST 12-2, DID 0x60, SDID 0x60) : heures, minutes, secondes,
/// images, et si c'est du drop-frame. None pour un autre paquet.
pub fn atc_timecode(p: &Packet) -> Option<(u8, u8, u8, u8, bool)> {
    if p.did != 0x60 || p.sdid != 0x60 || p.udw.len() < 16 {
        return None;
    }
    // Chaque mot porte un quartet (bits 4-7) : unités puis dizaines des images, secondes,
    // minutes, heures (mots 0, 2, 4, 6, 8, 10, 12, 14), les groupes utilisateurs entre.
    let q = |i: usize| (p.udw[i] >> 4) & 0xF;
    let frames = q(0) + (q(2) & 0x3) * 10;
    let drop = q(2) & 0x4 != 0;
    let secs = q(4) + (q(6) & 0x7) * 10;
    let mins = q(8) + (q(10) & 0x7) * 10;
    let hours = q(12) + (q(14) & 0x3) * 10;
    Some((hours, mins, secs, frames, drop))
}

/// Un paquet ATC (LTC, DBB1 = 0x00) pour ce timecode.
pub fn atc_packet(hours: u8, mins: u8, secs: u8, frames: u8, drop: bool) -> Packet {
    let digits = [frames % 10, (frames / 10 % 4) | if drop { 0x4 } else { 0 }, secs % 10, secs / 10 % 8, mins % 10, mins / 10 % 8, hours % 10, hours / 10 % 4];
    let mut udw = vec![0u8; 16];
    for (k, d) in digits.iter().enumerate() {
        udw[k * 2] = d << 4;
    }
    Packet { c: false, line: 9, offset: 0xFFF, did: 0x60, sdid: 0x60, udw, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_checks_the_words() {
        let a = atc_packet(10, 59, 58, 24, false);
        let b = Packet { c: true, line: 11, offset: 0xFFF, did: 0x61, sdid: 0x01, udw: (0..73).collect(), ..Default::default() };
        let g = encode(&[a.clone(), b.clone()], 2);
        // Les paquets sont alignés sur 32 bits dans la charge RTP (qui commence 2 octets plus tôt).
        assert_eq!(g.len() % 4, 2);
        let (ps, f) = decode(&g).unwrap();
        assert_eq!((ps, f), (vec![a.clone(), b], 2));
        assert_eq!(atc_timecode(&a), Some((10, 59, 58, 24, false)));
        // Parité : 0x60 a deux bits à 1 → b8 = 0, b9 = 1.
        assert_eq!(with_parity(0x60), 0x260);
        assert_eq!(with_parity(0x61), 0x161);
        // Rien : 6 octets, aucun paquet.
        let empty = encode(&[], 0);
        assert_eq!(empty, vec![0, 0, 0, 0, 0, 0]);
        assert_eq!(decode(&empty), Some((vec![], 0)));
        assert_eq!(decode(&[0, 200, 1, 0]), None);
    }

    #[test]
    fn matches_a_known_packet() {
        // Un paquet de RFC 8331 fait à la main : C=0, ligne 9, offset 0, S=0, flux 0,
        // DID 0x45, SDID 0x01, DC 1, UDW 0x10 (chacun un nombre impair de bits à 1 : b8 = 1).
        let p = Packet { line: 9, did: 0x45, sdid: 0x01, udw: vec![0x10], ..Default::default() };
        let g = encode(std::slice::from_ref(&p), 0);
        // C(0) Line(9) Offset(0) : 0 00000001001 000000000000 → 0x00 0x90 0x00
        assert_eq!(&g[6..9], &[0x00, 0x90, 0x00]);
        // S(0) StreamNum(0), puis DID 0x145 sur 10 bits.
        assert_eq!(g[9], 0x00);
        let w = |i: usize| {
            let bit = 10 * 8 + i * 10;
            let mut v = 0u16;
            for k in 0..10 {
                let b = bit + k;
                v = (v << 1) | ((g[b / 8] >> (7 - b % 8)) & 1) as u16;
            }
            v
        };
        assert_eq!(w(0), 0x145);
        assert_eq!(w(1), 0x101);
        assert_eq!(w(2), 0x101);
        assert_eq!(w(3), 0x110);
        // Somme sur 9 bits de 0x145 + 0x101 + 0x101 + 0x110 = 0x457 → 0x057 ; b9 = !b8 = 1.
        assert_eq!(w(4), 0x257);
    }
}
