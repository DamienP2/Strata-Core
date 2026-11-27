//! Une appli qui écrit une sortie en flux MXL (des barres qui défilent), comme le ferait LiveCut
//! pour son programme. Le Core l'envoie sur la sortie SDI et l'aperçu des réglages la montre.
//!
//! ```bash
//! cargo run -p strata-mxl --example send -- <identifiant du flux (mxlFlow)>
//! ```
use std::time::Duration;
use strata_mxl::{Mxl, Video};

fn main() -> Result<(), String> {
    let id: strata_mxl::Uuid = std::env::args().nth(1).ok_or("the flow id (mxlFlow of the output)")?.parse().map_err(|e| format!("{e}"))?;
    let mxl = Mxl::open()?;
    let v = Video { width: 1920, height: 1080, rate: (25, 1), interlaced: true };
    let mut w = mxl.video_writer(id, "Programme (exemple)", &v)?;
    const BARS: [[u8; 3]; 4] = [[180, 128, 128], [145, 54, 34], [63, 102, 240], [41, 240, 110]];
    let mut frame = vec![0u8; 1920 * 2 * 1080];
    for n in 0u64.. {
        for y in 0..1080 {
            for x in (0..1920).step_by(2) {
                let [yy, cb, cr] = BARS[((x as u64 + n * 8) / 480 % 4) as usize];
                let p = (y * 1920 + x) * 2;
                frame[p..p + 4].copy_from_slice(&[cb, yy, cr, yy]);
            }
        }
        w.write_uyvy(&frame, 1920 * 2)?;
        std::thread::sleep(Duration::from_millis(40));
    }
    Ok(())
}
