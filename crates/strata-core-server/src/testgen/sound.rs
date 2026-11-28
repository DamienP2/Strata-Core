//! Le son des mires (48 kHz, float) : un sinus (1 kHz, 400 Hz), du bruit rose ou du silence, au
//! niveau choisi, sur tous les canaux. L'identification des canaux : sur un cycle de 4 s, le
//! canal n est coupé n fois (100 ms coupé, 100 ms de son), ce qui le reconnaît à l'oreille et
//! sur un vumètre. La mire de calage coupe le son pendant l'éclair ; la mire Synchro AV fait
//! l'inverse : du silence, et un bip pendant l'éclair. En fréquence par canal, le canal n sonne à n
//! fois la fréquence choisie (reconnu à l'oreille, au spectre, et mesuré canal par canal).

pub const RATE: u64 = 48_000;
pub const TONES: &[&str] = &["1k", "400", "pink", "silence"];

pub struct Sound {
    tone: String,
    amp: f32,
    channels: usize,
    ident: bool,
    /// Le son seulement pendant l'éclair (un bip), du silence sinon.
    beep: bool,
    /// Le canal n à n fois la fréquence.
    per_channel: bool,
    /// L'échantillon suivant (depuis le départ).
    k: u64,
    noise: Pink,
}

impl Sound {
    pub fn new(tone: &str, level_db: f32, channels: usize, ident: bool) -> Sound {
        Sound {
            tone: tone.to_string(),
            amp: 10f32.powf(level_db / 20.0),
            channels: channels.max(1),
            ident,
            beep: false,
            per_channel: false,
            k: 0,
            noise: Pink::new(0x9e37_79b9),
        }
    }

    /// Le son seulement pendant l'éclair (mire Synchro AV).
    pub fn beeping(mut self, on: bool) -> Sound {
        self.beep = on;
        self
    }

    /// Le canal n à n fois la fréquence.
    pub fn per_channel(mut self, on: bool) -> Sound {
        self.per_channel = on;
        self
    }

    /// `n` échantillons entrelacés ; `flash` : l'image de l'éclair (coupé, ou le bip en Synchro AV).
    pub fn next(&mut self, n: usize, flash: bool) -> Vec<f32> {
        let mut out = Vec::with_capacity(n * self.channels);
        let base = if self.tone == "400" { 400 } else { 1000 };
        let on = if self.beep { flash } else { !flash };
        for i in 0..n {
            let k = self.k + i as u64;
            let noise = if self.tone == "pink" && on { self.noise.next() } else { 0.0 };
            for c in 0..self.channels {
                let f = if self.per_channel { base * (c as u64 + 1) } else { base };
                let v = if !on || self.tone == "silence" {
                    0.0
                } else if self.tone == "pink" {
                    noise
                } else if self.beep {
                    // Le bip part d'un demi-échantillon (pas de sin 0 = 0) : son premier
                    // échantillon s'entend, l'analyseur le date exactement.
                    let f = f.min(RATE / 2 - 1) as f32;
                    (2.0 * std::f32::consts::PI * f * (i as f32 + 0.5) / RATE as f32).sin()
                } else {
                    sine(k, f.min(RATE / 2 - 1))
                };
                out.push(if self.ident && cut(c, k) { 0.0 } else { self.amp * v });
            }
        }
        self.k += n as u64;
        out
    }
}

/// Un sinus de `f` Hz (entier) à l'échantillon `k`, exact sur la durée (phase en entiers).
fn sine(k: u64, f: u64) -> f32 {
    let ph = (k % RATE) * f % RATE;
    (2.0 * std::f32::consts::PI * ph as f32 / RATE as f32).sin()
}

/// L'identification : le canal `c` (0 = le premier) coupé c + 1 fois au début de chaque cycle de
/// 4 s.
fn cut(c: usize, k: u64) -> bool {
    let t = k % (4 * RATE);
    let slot = t / (RATE / 10);
    slot < 2 * (c as u64 + 1) && slot.is_multiple_of(2)
}

/// Du bruit rose (filtre de Paul Kellet sur un bruit blanc xorshift), crête autour de ±1.
struct Pink {
    s: u32,
    b: [f32; 7],
}

impl Pink {
    fn new(seed: u32) -> Pink {
        Pink { s: seed | 1, b: [0.0; 7] }
    }

    fn white(&mut self) -> f32 {
        self.s ^= self.s << 13;
        self.s ^= self.s >> 17;
        self.s ^= self.s << 5;
        self.s as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    fn next(&mut self) -> f32 {
        let w = self.white();
        let b = &mut self.b;
        b[0] = 0.99886 * b[0] + w * 0.0555179;
        b[1] = 0.99332 * b[1] + w * 0.0750759;
        b[2] = 0.96900 * b[2] + w * 0.153852;
        b[3] = 0.86650 * b[3] + w * 0.3104856;
        b[4] = 0.55000 * b[4] + w * 0.5329522;
        b[5] = -0.7616 * b[5] - w * 0.0168980;
        let p = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
        b[6] = w * 0.115926;
        (p * 0.11).clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_level_and_ident() {
        let mut s = Sound::new("1k", -18.0, 2, false);
        let pcm = s.next(4800, false);
        let peak = pcm.iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!((20.0 * peak.log10() + 18.0).abs() < 0.05, "{peak}");
        assert!(s.next(480, true).iter().all(|x| *x == 0.0));
        // Le canal 1 coupé une fois, le canal 3 trois fois, au début du cycle.
        assert!(cut(0, 0) && !cut(0, 4800) && !cut(0, 9600));
        assert!(cut(2, 9600 * 2) && !cut(2, 9600 * 3));
        let mut p = Sound::new("pink", 0.0, 1, false);
        assert!(p.next(48_000, false).iter().any(|x| x.abs() > 0.1));
        // Synchro AV : silence, puis le bip pendant l'éclair ; le canal 2 à 2 kHz.
        let mut b = Sound::new("1k", -18.0, 2, false).beeping(true).per_channel(true);
        assert!(b.next(960, false).iter().all(|x| *x == 0.0));
        let beep = b.next(960, true);
        assert!(beep.iter().any(|x| x.abs() > 0.1));
        // Canal 2 (2 kHz) : la période fait 24 échantillons.
        let ch2: Vec<f32> = beep.iter().skip(1).step_by(2).copied().collect();
        assert!((ch2[6] - ch2[30]).abs() < 1e-4 && (ch2[6] - ch2[18]).abs() > 0.1);
    }
}
