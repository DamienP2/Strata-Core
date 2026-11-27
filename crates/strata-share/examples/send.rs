//! Une appli qui envoie une sortie (SDI ou flux ST 2110) et publie ce qu'elle envoie pour
//! l'aperçu des réglages, seulement quand quelqu'un regarde.
//!
//! ```bash
//! STRATA_APP_ID=livecut cargo run -p strata-share --example send -- /dev/shm/strata-sdi-out-<carte>-2
//! ```
use std::time::{Duration, Instant};
use strata_share::{Format, FrameInfo, Timecode, Writer};

fn main() {
    let path = std::env::args().nth(1).expect("the share to write: /dev/shm/strata-sdi-out-<card>-<n>");
    let (w, h) = (1280u32, 720u32);
    let mut writer = Writer::create(path.as_ref(), 3, (w * h * 2) as usize).expect("cannot create the share");
    let mut frame = vec![0u8; (w * h * 2) as usize];
    let start = Instant::now();
    let mut n = 0u64;
    loop {
        // … l'image est envoyée sur la sortie ici, à chaque image …
        n += 1;
        // Publiée seulement si l'aperçu (ou une appli) la regarde.
        if writer.watched() {
            draw(&mut frame, w, h, n);
            let s = (n / 50) as u8;
            let info = FrameInfo {
                width: w,
                height: h,
                stride: w * 2,
                format: Format::Uyvy,
                rate: (50, 1),
                interlaced: false,
                timecode: Some(Timecode { hours: 10, minutes: 0, seconds: s % 60, frames: (n % 50) as u8, drop_frame: false, field_mark: false }),
            };
            let _ = writer.publish(&info, &frame);
        } else {
            writer.beat();
        }
        let next = start + Duration::from_millis(20 * n);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}

/// Des barres qui défilent.
fn draw(buf: &mut [u8], w: u32, h: u32, n: u64) {
    const BARS: [[u8; 3]; 4] = [[180, 128, 128], [145, 54, 34], [63, 102, 240], [41, 240, 110]];
    for y in 0..h as usize {
        for x in (0..w as usize).step_by(2) {
            let [yy, cb, cr] = BARS[((x as u64 + n * 8) / (w as u64 / 4) % 4) as usize];
            let p = (y * w as usize + x) * 2;
            buf[p..p + 4].copy_from_slice(&[cb, yy, cr, yy]);
        }
    }
}
