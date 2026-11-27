//! Essai : une appli qui écrit un bip (1 kHz, -18 dBFS, stéréo) dans un flux audio MXL.
//!
//! `cargo run -p strata-mxl --example tone -- <identifiant du flux (mxlFlow)>`
use std::time::{Duration, Instant};

fn main() -> Result<(), String> {
    let id: strata_mxl::Uuid = std::env::args().nth(1).ok_or("flow id")?.parse().map_err(|e| format!("{e}"))?;
    let mxl = strata_mxl::Mxl::open()?;
    let mut w = mxl.audio_writer(id, "Bip", 2)?;
    let amp = 10f32.powf(-18.0 / 20.0);
    let start = Instant::now();
    let mut n = 0u64;
    loop {
        let pcm: Vec<f32> = (0..480)
            .flat_map(|k| {
                let v = amp * (2.0 * std::f32::consts::PI * 1000.0 * (n + k) as f32 / 48000.0).sin();
                [v, v]
            })
            .collect();
        w.write_interleaved(&pcm)?;
        n += 480;
        let due = start + Duration::from_micros(n * 1_000_000 / 48000);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}
