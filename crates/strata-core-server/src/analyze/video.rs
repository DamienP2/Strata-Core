//! L'analyse de l'image (v210 10 bits) : forme d'onde de la luminance, vecteurscope, noir, image
//! figée, hors gamut. Sur un échantillon de l'image (une ligne sur quatre, des blocs de 6 pixels
//! répartis sur la largeur) : assez pour les scopes, peu coûteux.

pub const WAVE_COLS: usize = 96;
pub const WAVE_LEVELS: usize = 64;
pub const VEC: usize = 64;

/// Les composantes d'un bloc v210 de 6 pixels : [Cb0 Y0 Cr0 Y1 Cb1 Y2 Cr1 Y3 Cb2 Y4 Cr2 Y5].
fn block(b: &[u8]) -> [u16; 12] {
    let mut out = [0u16; 12];
    for w in 0..4 {
        let v = u32::from_le_bytes([b[w * 4], b[w * 4 + 1], b[w * 4 + 2], b[w * 4 + 3]]);
        for k in 0..3 {
            out[w * 3 + k] = ((v >> (10 * k)) & 0x3ff) as u16;
        }
    }
    out
}

/// La luminance (10 bits) d'un pixel d'une ligne v210.
pub fn luma_at(line: &[u8], x: usize) -> u16 {
    let b = x / 6;
    if line.len() < (b + 1) * 16 {
        return 0;
    }
    let c = block(&line[b * 16..b * 16 + 16]);
    [c[1], c[3], c[5], c[7], c[9], c[11]][x % 6]
}

#[derive(Default, serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Scopes {
    /// La forme d'onde : `WAVE_COLS` colonnes de `WAVE_LEVELS` niveaux (du bas vers le haut),
    /// 0 à 255 (échelle logarithmique), en base64.
    pub waveform: String,
    /// Le vecteurscope : `VEC` × `VEC` (Cb en x, Cr en y vers le haut), en base64.
    pub vector: String,
    /// La luminance moyenne (0 à 100 %).
    pub luma: f32,
    /// Noir : presque rien au-dessus du noir.
    pub black: bool,
    /// Les pixels hors des niveaux légaux (%).
    pub gamut: f32,
}

/// Une signature de l'image (des luminances échantillonnées) pour voir si elle change.
pub type Signature = Vec<u16>;

/// Les scopes d'une image v210 (`stride` octets par ligne, `h` lignes).
pub fn scopes(frame: &[u8], stride: usize, w: usize, h: usize) -> (Scopes, Signature) {
    use base64::Engine as _;
    let blocks = (w / 6).max(1);
    let mut wave = vec![0u32; WAVE_COLS * WAVE_LEVELS];
    let mut vec = vec![0u32; VEC * VEC];
    let (mut sum, mut n, mut bright, mut out) = (0u64, 0u64, 0u64, 0u64);
    let mut sig = Vec::with_capacity(WAVE_COLS * h / 16);
    for y in (0..h).step_by(4) {
        let Some(line) = frame.get(y * stride..y * stride + stride) else { break };
        for col in 0..WAVE_COLS {
            let b = col * blocks / WAVE_COLS;
            let Some(bytes) = line.get(b * 16..b * 16 + 16) else { continue };
            let c = block(bytes);
            for (i, &v) in c.iter().enumerate() {
                if i % 2 == 1 {
                    let level = (v as usize * WAVE_LEVELS / 1024).min(WAVE_LEVELS - 1);
                    wave[col * WAVE_LEVELS + level] += 1;
                    sum += v as u64;
                    n += 1;
                    if v > 160 {
                        bright += 1;
                    }
                    if !(64..=940).contains(&v) {
                        out += 1;
                    }
                } else if !(64..=960).contains(&v) {
                    out += 1;
                }
            }
            for k in 0..3 {
                let (cb, cr) = (c[k * 4], c[k * 4 + 2]);
                let x = (cb as usize * VEC / 1024).min(VEC - 1);
                let yy = VEC - 1 - (cr as usize * VEC / 1024).min(VEC - 1);
                vec[yy * VEC + x] += 1;
            }
            if y % 16 == 0 {
                sig.push(c[3]);
            }
        }
    }
    let norm = |v: &[u32]| -> Vec<u8> {
        let max = v.iter().copied().max().unwrap_or(1).max(1) as f32;
        v.iter().map(|&x| if x == 0 { 0 } else { (40.0 + 215.0 * (x as f32).ln_1p() / max.ln_1p()) as u8 }).collect()
    };
    let e = base64::engine::general_purpose::STANDARD;
    let luma = if n > 0 { ((sum as f32 / n as f32) - 64.0) / 876.0 * 100.0 } else { 0.0 };
    let s = Scopes {
        waveform: e.encode(norm(&wave)),
        vector: e.encode(norm(&vec)),
        luma: luma.clamp(0.0, 110.0),
        black: n > 0 && (bright as f64) < n as f64 * 0.002,
        gamut: if n > 0 { out as f32 * 100.0 / (n as f32 * 2.0) } else { 0.0 },
    };
    (s, sig)
}

/// L'écart moyen entre deux signatures (0 : la même image).
pub fn difference(a: &Signature, b: &Signature) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return f32::MAX;
    }
    a.iter().zip(b).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs()).sum::<u32>() as f32 / a.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_of(uyvy: &[u8], w: usize) -> Vec<u8> {
        let mut l = vec![0u8; strata_mxl::v210_stride(w as u32)];
        strata_mxl::uyvy_to_v210(uyvy, &mut l, w);
        l
    }

    #[test]
    fn black_and_grey() {
        let w = 192;
        let black: Vec<u8> = (0..w * 2).map(|i| if i % 2 == 0 { 128 } else { 16 }).collect();
        let l = line_of(&black, w);
        let frame = l.repeat(16);
        let (s, sig) = scopes(&frame, l.len(), w, 16);
        assert!(s.black && s.luma < 1.0 && s.gamut < 0.01, "{}", s.luma);
        assert_eq!(luma_at(&l, 7), 64);
        let grey: Vec<u8> = (0..w * 2).map(|i| if i % 2 == 0 { 128 } else { 126 }).collect();
        let g = line_of(&grey, w).repeat(16);
        let (s2, sig2) = scopes(&g, l.len(), w, 16);
        assert!(!s2.black && (s2.luma - 50.0).abs() < 2.0, "{}", s2.luma);
        assert!(difference(&sig, &sig2) > 100.0);
        assert_eq!(difference(&sig2, &sig2), 0.0);
    }
}
