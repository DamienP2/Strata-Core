//! Essai : suit un flux audio MXL (`audio_probe <identifiant>`) et dit où en est son écrivain.
fn main() -> Result<(), String> {
    let id: strata_mxl::Uuid = std::env::args().nth(1).ok_or("flow id")?.parse().map_err(|e| format!("{e}"))?;
    let mxl = strata_mxl::Mxl::open()?;
    let mut r = mxl.audio_reader(id)?;
    let mut last = r.written().unwrap_or(0);
    let t0 = std::time::Instant::now();
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(40));
        let w = r.written().unwrap_or(0);
        let got = r.next_interleaved(1920).is_some();
        println!(
            "{:>5} ms  written-now {:>7} ms  +{}  read {}",
            t0.elapsed().as_millis(),
            (w as i64 - r.now() as i64) / 48,
            w as i64 - last as i64,
            if got { "yes" } else { "NO" }
        );
        last = w;
    }
    Ok(())
}
