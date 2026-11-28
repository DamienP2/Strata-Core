//! Les images des mires, en UYVY 8 bits (BT.709, niveaux légaux) : la mire elle-même (calculée
//! une fois), puis ce qui change à chaque image (identification, timecode, horloge, repère animé).

/// Une couleur Y, Cb, Cr (8 bits, niveaux légaux) d'après R, G, B entre 0 et 1 (BT.709).
pub fn ycc(r: f32, g: f32, b: f32) -> [u8; 3] {
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let cb = (b - y) / 1.8556;
    let cr = (r - y) / 1.5748;
    [
        (16.0 + 219.0 * y).round().clamp(1.0, 254.0) as u8,
        (128.0 + 224.0 * cb).round().clamp(1.0, 254.0) as u8,
        (128.0 + 224.0 * cr).round().clamp(1.0, 254.0) as u8,
    ]
}

/// Un gris (0 à 1 : du noir au blanc).
pub fn grey(l: f32) -> [u8; 3] {
    ycc(l, l, l)
}

/// Une image UYVY de `w` × `h` (largeur paire).
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn new(w: usize, h: usize, c: [u8; 3]) -> Frame {
        let mut f = Frame { w, h, data: vec![0; w * h * 2] };
        f.fill(0, 0, w, h, c);
        f
    }

    pub fn stride(&self) -> usize {
        self.w * 2
    }

    /// Un rectangle plein (bornes rognées ; x arrondi aux paires de pixels).
    pub fn fill(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, [y, cb, cr]: [u8; 3]) {
        let (x0, x1) = (x0.min(self.w) & !1, x1.min(self.w).div_ceil(2) * 2);
        let (y0, y1) = (y0.min(self.h), y1.min(self.h));
        if x0 >= x1 {
            return;
        }
        let quad = [cb, y, cr, y];
        for row in y0..y1 {
            let line = &mut self.data[row * self.w * 2..(row + 1) * self.w * 2];
            for q in line[x0 * 2..x1 * 2].as_chunks_mut::<4>().0 {
                q.copy_from_slice(&quad);
            }
        }
    }

    /// Un pixel de luminance seule (chrominance neutre), pour les dégradés et la zone plate.
    fn set_luma_pair(&mut self, x: usize, row: usize, y0: u8, y1: u8) {
        let at = row * self.w * 2 + x * 2;
        self.data[at..at + 4].copy_from_slice(&[128, y0, 128, y1]);
    }
}

/// Les mires.
pub const PATTERNS: &[&str] = &["bars75", "bars100", "smpte", "ramp", "checker", "zoneplate", "solid", "black", "lipsync"];

/// La mire de base (sans ce qui change d'une image à l'autre). `color` : la couleur unie (#RRGGBB).
pub fn pattern(name: &str, w: usize, h: usize, color: &str) -> Frame {
    match name {
        "bars100" => bars(w, h, 1.0),
        "smpte" => smpte(w, h),
        "ramp" => ramp(w, h),
        "checker" => checker(w, h),
        "zoneplate" => zoneplate(w, h),
        "solid" => {
            let (r, g, b) = hex(color).unwrap_or((0.5, 0.5, 0.5));
            Frame::new(w, h, ycc(r, g, b))
        }
        "black" => Frame::new(w, h, grey(0.0)),
        // La mire de calage : un fond gris sombre, l'éclair blanc vient image par image.
        "lipsync" => Frame::new(w, h, grey(0.2)),
        _ => bars(w, h, 0.75),
    }
}

/// `#RRGGBB` en R, G, B entre 0 et 1.
pub fn hex(s: &str) -> Option<(f32, f32, f32)> {
    let s = s.trim().strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok().map(|x| x as f32 / 255.0);
    Some((v(0)?, v(2)?, v(4)?))
}

/// Huit couleurs des barres : blanc, jaune, cyan, vert, magenta, rouge, bleu (et noir).
const BARS: [(f32, f32, f32); 7] = [(1.0, 1.0, 1.0), (1.0, 1.0, 0.0), (0.0, 1.0, 1.0), (0.0, 1.0, 0.0), (1.0, 0.0, 1.0), (1.0, 0.0, 0.0), (0.0, 0.0, 1.0)];

fn bars(w: usize, h: usize, level: f32) -> Frame {
    let mut f = Frame::new(w, h, grey(0.0));
    for (i, (r, g, b)) in BARS.iter().enumerate() {
        f.fill(w * i / 7, 0, w * (i + 1) / 7, h, ycc(r * level, g * level, b * level));
    }
    f
}

/// La mire SMPTE RP 219 (HD), simplifiée : barres à 75 % entre deux gris à 40 %, une ligne cyan /
/// blanc 100 % / bleu, une ligne jaune / rampe / rouge, puis noir, blanc et les PLUGE (−2 %, 0,
/// +2 %, +4 %).
fn smpte(w: usize, h: usize) -> Frame {
    let mut f = Frame::new(w, h, grey(0.0));
    let side = w / 8;
    let (h1, h2, h3) = (h * 7 / 12, h * 8 / 12, h * 9 / 12);
    let inner = w - 2 * side;
    f.fill(0, 0, side, h1, grey(0.4));
    f.fill(w - side, 0, w, h1, grey(0.4));
    for (i, (r, g, b)) in BARS.iter().enumerate() {
        f.fill(side + inner * i / 7, 0, side + inner * (i + 1) / 7, h1, ycc(r * 0.75, g * 0.75, b * 0.75));
    }
    f.fill(0, h1, side, h2, ycc(0.0, 1.0, 1.0));
    f.fill(side, h1, side + inner / 7, h2, grey(1.0));
    f.fill(side + inner / 7, h1, w - side, h2, grey(0.75));
    f.fill(w - side, h1, w, h2, ycc(0.0, 0.0, 1.0));
    f.fill(0, h2, side, h3, ycc(1.0, 1.0, 0.0));
    f.fill(w - side, h2, w, h3, ycc(1.0, 0.0, 0.0));
    for x in (side..w - side).step_by(2) {
        let l = (x - side) as f32 / inner as f32;
        let y = grey(l)[0];
        for row in h2..h3 {
            f.set_luma_pair(x, row, y, y);
        }
    }
    f.fill(0, h3, side, h, grey(0.15));
    f.fill(w - side, h3, w, h, grey(0.15));
    let u = inner / 12;
    f.fill(side + u, h3, side + 4 * u, h, grey(1.0));
    // PLUGE : −2 %, 0, +2 %, 0, +4 % (en niveaux de luminance autour du noir à 16).
    let pluge = [(-2.0f32, 6), (0.0, 7), (2.0, 8), (0.0, 9), (4.0, 10)];
    for (pct, k) in pluge {
        let y = (16.0 + 219.0 * pct / 100.0).round() as u8;
        f.fill(side + k * u, h3, side + (k + 1) * u, h, [y, 128, 128]);
    }
    f
}

fn ramp(w: usize, h: usize) -> Frame {
    let mut f = Frame::new(w, h, grey(0.0));
    for x in (0..w).step_by(2) {
        let y0 = grey(x as f32 / (w - 1).max(1) as f32)[0];
        let y1 = grey((x + 1) as f32 / (w - 1).max(1) as f32)[0];
        for row in 0..h {
            f.set_luma_pair(x, row, y0, y1);
        }
    }
    f
}

fn checker(w: usize, h: usize) -> Frame {
    let mut f = Frame::new(w, h, grey(0.0));
    let s = (h / 9).max(2);
    for by in 0..h.div_ceil(s) {
        for bx in 0..w.div_ceil(s) {
            if (bx + by) % 2 == 0 {
                f.fill(bx * s, by * s, (bx + 1) * s, (by + 1) * s, grey(1.0));
            }
        }
    }
    f
}

/// Une zone plate circulaire : la fréquence croît du centre (0) au bord de la hauteur (Nyquist).
fn zoneplate(w: usize, h: usize) -> Frame {
    let mut f = Frame::new(w, h, grey(0.5));
    let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, h as f32 / 2.0);
    let luma = |x: usize, y: usize| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let phase = std::f32::consts::PI * (dx * dx + dy * dy) / (2.0 * r);
        (16.0 + 219.0 * (0.5 + 0.5 * phase.cos())).round() as u8
    };
    for row in 0..h {
        for x in (0..w).step_by(2) {
            let (a, b) = (luma(x, row), luma(x + 1, row));
            f.set_luma_pair(x, row, a, b);
        }
    }
    f
}

// ---------- Le texte incrusté ----------

/// Une police 5 × 7 (une ligne par octet, bit 4 à gauche).
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        ';' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x04, 0x08],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '·' => [0x00, 0x00, 0x00, 0x0C, 0x0C, 0x00, 0x00],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '#' => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '\'' | '’' => [0x0C, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        _ => [0; 7],
    }
}

/// Une lettre accentuée ou en minuscule ramenée à ce que la police connaît.
fn plain(c: char) -> char {
    let u = c.to_uppercase().next().unwrap_or(c);
    match u {
        'À' | 'Â' | 'Ä' | 'Á' | 'Ã' => 'A',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'Î' | 'Ï' | 'Í' => 'I',
        'Ô' | 'Ö' | 'Ó' | 'Õ' => 'O',
        'Ù' | 'Û' | 'Ü' | 'Ú' => 'U',
        'Ç' => 'C',
        'Ñ' => 'N',
        'Ÿ' => 'Y',
        _ => u,
    }
}

/// La taille d'un point de la police pour cette hauteur d'image.
pub fn scale(h: usize) -> usize {
    (h / 135).max(2)
}

/// La largeur d'un texte (en pixels) à cette échelle.
pub fn text_width(text: &str, s: usize) -> usize {
    text.chars().count() * 6 * s
}

/// Un texte blanc sur un pavé noir, son coin haut gauche en (x, y).
pub fn label(f: &mut Frame, x: usize, y: usize, text: &str, s: usize) {
    let (tw, th) = (text_width(text, s), 7 * s);
    let pad = 2 * s;
    f.fill(x.saturating_sub(pad), y.saturating_sub(pad), x + tw + pad - s, y + th + pad, grey(0.0));
    for (i, c) in text.chars().enumerate() {
        let g = glyph(plain(c));
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5 {
                if bits & (0x10 >> col) != 0 {
                    let px = x + (i * 6 + col) * s;
                    let py = y + row * s;
                    f.fill(px, py, px + s, py + s, grey(1.0));
                }
            }
        }
    }
}

/// Un texte centré horizontalement, à la hauteur `y`.
pub fn label_centred(f: &mut Frame, y: usize, text: &str, s: usize) {
    let x = f.w.saturating_sub(text_width(text, s)) / 2;
    label(f, x, y, text, s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_legal() {
        assert_eq!(grey(0.0), [16, 128, 128]);
        assert_eq!(grey(1.0), [235, 128, 128]);
        // Le jaune à 75 % de la mire EBU (BT.709).
        let y = ycc(0.75, 0.75, 0.0);
        assert!((y[0] as i32 - 168).abs() <= 1, "{y:?}");
    }

    #[test]
    fn patterns_fill_the_frame() {
        for p in PATTERNS {
            let f = pattern(p, 1280, 720, "#336699");
            assert_eq!(f.data.len(), 1280 * 720 * 2, "{p}");
            assert!(f.data.as_chunks::<2>().0.iter().all(|c| c[1] >= 1 && c[1] <= 254), "{p}");
        }
        // Barres : le blanc à gauche, le bleu à droite.
        let b = pattern("bars75", 700, 10, "");
        assert_eq!(b.data[1], ycc(0.75, 0.75, 0.75)[0]);
        assert_eq!(b.data[699 * 2 - 1], ycc(0.0, 0.0, 0.75)[0]);
    }

    #[test]
    fn text_is_drawn() {
        let mut f = Frame::new(400, 100, grey(0.5));
        label(&mut f, 10, 10, "TC 10:00", 2);
        assert!(f.data.as_chunks::<2>().0.iter().any(|c| c[1] == 235));
        assert!(f.data.as_chunks::<2>().0.iter().any(|c| c[1] == 16));
        assert_eq!(plain('é'), 'E');
    }
}
