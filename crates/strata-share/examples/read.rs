//! Lit une entrée partagée comme le ferait une appli, et dit ce qu'elle reçoit chaque seconde.
//!
//! ```bash
//! STRATA_APP_ID=livecut cargo run -p strata-share --example read -- /dev/shm/strata-sdi-<carte>-1
//! ```
use std::time::{Duration, Instant};
use strata_share::{Next, Reader};

fn main() {
    let path = std::env::args().nth(1).expect("the share to read: /dev/shm/strata-sdi-<card>-<n>");
    let app = std::env::var("STRATA_APP_ID").unwrap_or_else(|_| "example".into());
    let mut buf = Vec::new();
    loop {
        // Le Core n'a pas (encore) créé l'entrée, ou l'a recréée : on rouvre.
        let mut r = match Reader::open(path.as_ref(), &app) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("waiting for {path}: {e}");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let (mut count, mut since) = (0u32, Instant::now());
        loop {
            match r.next(&mut buf, Duration::from_secs(1)) {
                Next::Frame(f) => {
                    count += 1;
                    if since.elapsed() >= Duration::from_secs(1) {
                        let tc = f.info.timecode.map(|t| format!("{:02}:{:02}:{:02}:{:02}", t.hours, t.minutes, t.seconds, t.frames));
                        println!("frame {} · {}x{} · {count} fps · TC {}", f.no, f.info.width, f.info.height, tc.unwrap_or_else(|| "—".into()));
                        (count, since) = (0, Instant::now());
                    }
                }
                Next::Timeout => println!("no picture (signal: {:?})", r.signal()),
                Next::Gone => break,
            }
        }
    }
}
