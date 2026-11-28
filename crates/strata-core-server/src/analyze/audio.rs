//! La mesure du son : crête et RMS par canal, sonie EBU R128 (ITU-R BS.1770-4 : pondération K,
//! momentanée 400 ms, court terme 3 s, intégrée avec ses deux portes, plage LRA), crête vraie
//! (suréchantillonnage ×4), corrélation de phase d'une paire.

/// Un biquad (forme directe II transposée).
#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
    z: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Biquad {
        Biquad { b, a, z: [0.0; 2] }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[1] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[2] * y;
        y
    }
}

/// La pondération K à 48 kHz (BS.1770-4, tableaux 1 et 2).
fn k_filter() -> [Biquad; 2] {
    [
        Biquad::new([1.53512485958697, -2.69169618940638, 1.19839281085285], [1.0, -1.69065929318241, 0.73248077421585]),
        Biquad::new([1.0, -2.0, 1.0], [1.0, -1.99004745483398, 0.99007225036621]),
    ]
}

fn lufs(mean_square: f64) -> f64 {
    if mean_square <= 0.0 {
        f64::NEG_INFINITY
    } else {
        -0.691 + 10.0 * mean_square.log10()
    }
}

/// Le suréchantillonnage ×4 de la crête vraie : un filtre de 48 coefficients (sinus cardinal
/// fenêtré, coupure au quart), en 4 phases de 12.
struct TruePeak {
    h: [[f32; 12]; 4],
    hist: Vec<[f32; 12]>,
    peak: f32,
}

impl TruePeak {
    fn new(channels: usize) -> TruePeak {
        let mut h = [[0f32; 12]; 4];
        let n = 48;
        let mut sum = [0f64; 4];
        for (i, slot) in (0..n).map(|i| (i, i % 4)) {
            let t = i as f64 - (n as f64 - 1.0) / 2.0;
            let x = std::f64::consts::PI * t / 4.0;
            let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n as f64 - 1.0)).cos();
            h[slot][i / 4] = (sinc * w) as f32;
            sum[slot] += sinc * w;
        }
        // Chaque phase a un gain de 1 (un signal continu ressort à son niveau).
        for (p, s) in sum.iter().enumerate() {
            for v in h[p].iter_mut() {
                *v /= *s as f32;
            }
        }
        TruePeak { h, hist: vec![[0.0; 12]; channels], peak: 0.0 }
    }

    fn push(&mut self, c: usize, x: f32) {
        let hist = &mut self.hist[c];
        hist.copy_within(0..11, 1);
        hist[0] = x;
        for ph in &self.h {
            let y: f32 = ph.iter().zip(hist.iter()).map(|(a, b)| a * b).sum();
            self.peak = self.peak.max(y.abs());
        }
    }
}

/// Le niveau d'un canal sur le dernier intervalle.
#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct Level {
    /// Crête (dBFS).
    pub peak: f32,
    /// RMS (dBFS).
    pub rms: f32,
}

#[derive(Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loudness {
    pub momentary: Option<f64>,
    pub short_term: Option<f64>,
    pub integrated: Option<f64>,
    pub lra: Option<f64>,
    /// Crête vraie depuis la remise à zéro (dBTP).
    pub true_peak: Option<f64>,
    /// Les canaux mesurés (1 = le premier).
    pub channels: Vec<usize>,
}

fn db(x: f32) -> f32 {
    if x <= 1e-9 {
        -120.0
    } else {
        20.0 * x.log10()
    }
}

/// La mesure d'un groupe de canaux.
pub struct Meter {
    pub channels: usize,
    // Crête et somme des carrés depuis la dernière lecture, par canal.
    peak: Vec<f32>,
    sq: Vec<f64>,
    n: u64,
    // Sonie : les canaux mesurés, leurs filtres, les blocs de 100 ms.
    group: Vec<usize>,
    k: Vec<[Biquad; 2]>,
    block_sq: f64,
    block_n: usize,
    blocks: std::collections::VecDeque<f64>,
    /// Les blocs de 400 ms depuis la remise à zéro (intégrée).
    gated: Vec<f64>,
    /// Les valeurs court terme depuis la remise à zéro, une par seconde (LRA).
    short: Vec<f64>,
    since_short: usize,
    tp: TruePeak,
    tp_max: f32,
    /// Les derniers échantillons de la paire mesurée (spectre, goniomètre).
    pair: std::collections::VecDeque<(f32, f32)>,
    // Phase de la paire 1-2.
    lr: f64,
    ll: f64,
    rr: f64,
    pub phase: Option<f64>,
}

impl Meter {
    pub fn new(channels: usize, group: Vec<usize>) -> Meter {
        let group: Vec<usize> = group.into_iter().filter(|c| *c < channels).collect();
        Meter {
            channels,
            peak: vec![0.0; channels],
            sq: vec![0.0; channels],
            n: 0,
            k: group.iter().map(|_| k_filter()).collect(),
            tp: TruePeak::new(group.len()),
            group,
            block_sq: 0.0,
            block_n: 0,
            blocks: Default::default(),
            gated: Vec::new(),
            short: Vec::new(),
            since_short: 0,
            tp_max: 0.0,
            pair: Default::default(),
            lr: 0.0,
            ll: 0.0,
            rr: 0.0,
            phase: None,
        }
    }

    /// Les canaux mesurés (sonie, crête vraie, spectre, goniomètre), 0 = le premier.
    pub fn group(&self) -> &[usize] {
        &self.group
    }

    /// D'autres canaux mesurés : la sonie repart de zéro.
    pub fn set_group(&mut self, group: Vec<usize>) {
        let channels = self.channels;
        *self = Meter::new(channels, group);
    }

    /// Le spectre de la paire mesurée : 31 bandes d'un tiers d'octave (20 Hz à 20 kHz), en dB
    /// sous un sinus à pleine échelle.
    pub fn spectrum(&self) -> Vec<f32> {
        const N: usize = 2048;
        if self.pair.len() < N {
            return Vec::new();
        }
        let mut re: Vec<f32> = Vec::with_capacity(N);
        for (i, (l, r)) in self.pair.iter().skip(self.pair.len() - N).enumerate() {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos();
            re.push((l + r) * 0.5 * w);
        }
        let mut im = vec![0f32; N];
        fft(&mut re, &mut im);
        let full = (N as f32 / 4.0).powi(2) * 1.5;
        (0..31)
            .map(|k| {
                let fc = 1000.0 * 2f32.powf((k as f32 - 17.0) / 3.0);
                let (lo, hi) = (fc / 2f32.powf(1.0 / 6.0), fc * 2f32.powf(1.0 / 6.0));
                let (a, b) = (((lo * N as f32 / 48_000.0).floor() as usize).max(1), ((hi * N as f32 / 48_000.0).ceil() as usize).min(N / 2));
                let p: f32 = (a..b.max(a + 1)).map(|i| re[i] * re[i] + im[i] * im[i]).sum();
                (10.0 * (p / full).max(1e-12).log10()).max(-90.0)
            })
            .collect()
    }

    /// Le goniomètre : 256 instants de la paire mesurée (gauche, droite).
    pub fn gonio(&self) -> Vec<[f32; 2]> {
        let step = (self.pair.len() / 256).max(1);
        self.pair.iter().step_by(step).map(|(l, r)| [(l * 1000.0).round() / 1000.0, (r * 1000.0).round() / 1000.0]).collect()
    }

    /// Remet la sonie intégrée, la plage et la crête vraie à zéro.
    pub fn reset(&mut self) {
        self.gated.clear();
        self.short.clear();
        self.tp_max = 0.0;
    }

    /// Des échantillons entrelacés (`channels` par instant).
    pub fn push(&mut self, pcm: &[f32]) {
        let ch = self.channels.max(1);
        for frame in pcm.chunks_exact(ch) {
            for (c, x) in frame.iter().enumerate() {
                self.peak[c] = self.peak[c].max(x.abs());
                self.sq[c] += (*x as f64) * (*x as f64);
            }
            self.n += 1;
            // Sonie : somme des carrés pondérés K (tous les canaux à 1 : stéréo, mono).
            let mut s = 0.0;
            for (i, &c) in self.group.iter().enumerate() {
                let x = frame[c] as f64;
                let [k0, k1] = &mut self.k[i];
                let y = k1.run(k0.run(x));
                s += y * y;
                self.tp.push(i, frame[c]);
            }
            self.block_sq += s;
            self.block_n += 1;
            if self.block_n == 4_800 {
                self.close_block();
            }
            // La paire mesurée (un seul canal : aux deux côtés).
            let (pl, pr) = match self.group.as_slice() {
                [] => (frame[0], frame[0]),
                [a] => (frame[*a], frame[*a]),
                [a, b, ..] => (frame[*a], frame[*b]),
            };
            self.pair.push_back((pl, pr));
            if self.pair.len() > 4096 {
                self.pair.pop_front();
            }
            if ch >= 2 {
                let (l, r) = (frame[0] as f64, frame[1] as f64);
                self.lr += l * r;
                self.ll += l * l;
                self.rr += r * r;
            }
        }
    }

    fn close_block(&mut self) {
        self.blocks.push_back(self.block_sq / self.block_n as f64);
        self.block_sq = 0.0;
        self.block_n = 0;
        while self.blocks.len() > 30 {
            self.blocks.pop_front();
        }
        if self.blocks.len() >= 4 {
            let m: f64 = self.blocks.iter().rev().take(4).sum::<f64>() / 4.0;
            self.gated.push(m);
            if self.gated.len() > 36_000 {
                self.gated.remove(0);
            }
        }
        self.since_short += 1;
        if self.blocks.len() >= 30 && self.since_short >= 10 {
            self.since_short = 0;
            self.short.push(lufs(self.blocks.iter().sum::<f64>() / 30.0));
        }
        self.tp_max = self.tp_max.max(self.tp.peak);
        self.tp.peak = 0.0;
    }

    /// Les niveaux depuis la dernière lecture (puis remis à zéro).
    pub fn take_levels(&mut self) -> Vec<Level> {
        let n = self.n.max(1) as f64;
        let v = (0..self.channels).map(|c| Level { peak: db(self.peak[c]), rms: db((self.sq[c] / n).sqrt() as f32) }).collect();
        self.peak.iter_mut().for_each(|p| *p = 0.0);
        self.sq.iter_mut().for_each(|s| *s = 0.0);
        self.n = 0;
        if self.ll > 0.0 && self.rr > 0.0 {
            let c = self.lr / (self.ll * self.rr).sqrt();
            self.phase = Some(match self.phase {
                Some(p) => p * 0.6 + c * 0.4,
                None => c,
            });
        }
        (self.lr, self.ll, self.rr) = (0.0, 0.0, 0.0);
        v
    }

    pub fn loudness(&self) -> Loudness {
        let last = |k: usize| (self.blocks.len() >= k).then(|| lufs(self.blocks.iter().rev().take(k).sum::<f64>() / k as f64));
        // Intégrée : porte absolue −70 LUFS, puis relative −10 LU sous la moyenne des blocs gardés.
        let abs: Vec<f64> = self.gated.iter().copied().filter(|m| lufs(*m) > -70.0).collect();
        let integrated = (!abs.is_empty()).then(|| {
            let rel = lufs(abs.iter().sum::<f64>() / abs.len() as f64) - 10.0;
            let kept: Vec<f64> = abs.iter().copied().filter(|m| lufs(*m) > rel).collect();
            lufs(kept.iter().sum::<f64>() / kept.len().max(1) as f64)
        });
        // LRA : valeurs court terme, porte absolue −70, relative −20 LU ; du 10e au 95e centile.
        let mut st: Vec<f64> = self.short.iter().copied().filter(|l| *l > -70.0).collect();
        let lra = (st.len() >= 2).then(|| {
            let mean = 10.0 * (st.iter().map(|l| 10f64.powf(l / 10.0)).sum::<f64>() / st.len() as f64).log10();
            st.retain(|l| *l > mean - 20.0);
            st.sort_by(|a, b| a.total_cmp(b));
            let at = |p: f64| st[((st.len() - 1) as f64 * p).round() as usize];
            at(0.95) - at(0.10)
        });
        Loudness {
            momentary: last(4).filter(|v| v.is_finite()),
            short_term: last(30).filter(|v| v.is_finite()),
            integrated: integrated.filter(|v| v.is_finite()),
            lra,
            true_peak: (self.tp_max > 0.0).then(|| 20.0 * (self.tp_max as f64).log10()),
            channels: self.group.iter().map(|c| c + 1).collect(),
        }
    }
}

/// Une FFT complexe en place (radix 2, `re.len()` puissance de deux).
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1f32, 0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, f: f64, amp: f64, ch: usize) -> Vec<f32> {
        (0..n).flat_map(|k| std::iter::repeat_n((amp * (2.0 * std::f64::consts::PI * f * k as f64 / 48_000.0).sin()) as f32, ch)).collect()
    }

    #[test]
    fn levels_and_phase() {
        let mut m = Meter::new(2, vec![0, 1]);
        m.push(&sine(4_800, 1000.0, 0.125_892_54, 2));
        let l = m.take_levels();
        assert!((l[0].peak + 18.0).abs() < 0.05, "{}", l[0].peak);
        assert!((l[0].rms + 21.01).abs() < 0.1, "{}", l[0].rms);
        assert!((m.phase.unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn loudness_of_a_reference_tone() {
        // BS.1770 : un sinus 1 kHz à −18 dBFS sur un canal donne −18 + 0,69 − 3,01 ≈ −21 LUFS ;
        // sur deux canaux, 3 dB de plus.
        let mut m = Meter::new(2, vec![0, 1]);
        m.push(&sine(48_000 * 4, 1000.0, 0.125_892_54, 2));
        let l = m.loudness();
        let i = l.integrated.unwrap();
        assert!((i + 18.0).abs() < 0.2, "{i}");
        assert!((l.momentary.unwrap() + 18.0).abs() < 0.2);
        let tp = l.true_peak.unwrap();
        assert!((tp + 18.0).abs() < 0.3, "{tp}");
        m.reset();
        assert!(m.loudness().integrated.is_none());
    }

    #[test]
    fn spectrum_of_a_tone() {
        let mut m = Meter::new(2, vec![0, 1]);
        m.push(&sine(4_096, 1000.0, 1.0, 2));
        let sp = m.spectrum();
        assert_eq!(sp.len(), 31);
        // La bande de 1 kHz (la 18e) près de 0 dB, les autres bien plus bas.
        assert!(sp[17] > -2.0 && sp[17] < 1.0, "{}", sp[17]);
        assert!(sp[5] < -40.0 && sp[28] < -40.0, "{:?}", sp);
        assert_eq!(m.gonio().len(), 256);
    }
}
