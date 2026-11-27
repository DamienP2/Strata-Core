//! Essai : suit un flux audio MXL (`audio_probe <identifiant>`) : où en est son écrivain, et le
//! niveau de ce qui est lu.
fn main() -> Result<(), String> {
    let id: strata_mxl::Uuid = std::env::args().nth(1).ok_or("flow id")?.parse().map_err(|e| format!("{e}"))?;
    let mxl = strata_mxl::Mxl::open()?;
    let mut r = mxl.audio_reader(id)?;
    let mut last = r.written().unwrap_or(0);
    let t0 = std::time::Instant::now();
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(40));
        let w = r.written().unwrap_or(0);
        let got = r.next_interleaved(1920);
        let level = got.as_ref().map(|s| 20.0 * (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt().max(1e-9).log10());
        println!(
            "{:>5} ms  written-now {:>5} ms  +{:<5}  read {}",
            t0.elapsed().as_millis(),
            (w as i64 - r.now() as i64) / 48,
            w as i64 - last as i64,
            level.map_or("NO".to_string(), |db| format!("{db:.1} dBFS"))
        );
        last = w;
    }
    Ok(())
}
