//! Essai : un flux ST 2110-40 (un paquet ATC par image, le timecode qui avance) envoyé et reçu par
//! le même port (socket du noyau).
//!
//! `STRATA_MTL_LIB=…/libmtl.so cargo run -p strata-mtl --example anc_loopback -- [i]`
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use strata_mtl::{load, Anc, Leg, Mtl, Pmd, Port, Video};

fn main() -> Result<(), String> {
    let interlaced = std::env::args().nth(1).is_some_and(|a| a == "i");
    println!("libmtl: {}", load()?.display());
    let ip = Ipv4Addr::LOCALHOST;
    let port = Port { name: "kernel:lo".into(), pmd: Pmd::Kernel, ip, mask: Ipv4Addr::new(255, 0, 0, 0), gateway: Ipv4Addr::UNSPECIFIED };
    let mtl = Mtl::open(&[port], 37, None)?;
    let v = Video { width: 1920, height: 1080, rate: (25, 1), interlaced };
    let leg = [Leg { port: 0, ip, udp: 20040 }];
    let rx = mtl.drx("anc-rx", &leg, 0, interlaced)?;
    let tx = std::sync::Arc::new(mtl.dtx("anc-tx", &leg, 100, &v)?);
    let t2 = tx.clone();
    let sender = std::thread::spawn(move || {
        let start = Instant::now();
        let mut n = 0u32;
        while start.elapsed() < Duration::from_secs(4) {
            // Un paquet ATC : le numéro d'image dans les unités d'images (mot 0, bits 4-7).
            let mut udw = vec![0u8; 16];
            udw[0] = ((n % 10) as u8) << 4;
            let a = Anc { line: 9, offset: 0xFFF, did: 0x60, sdid: 0x60, udw, ..Default::default() };
            if t2.send(&[a]) {
                n += 1;
            }
        }
        n
    });
    let start = Instant::now();
    let (mut got, mut fields) = (0u32, [0u32; 4]);
    while start.elapsed() < Duration::from_secs(4) {
        if let Some((p, f)) = rx.next() {
            got += 1;
            fields[f as usize & 3] += 1;
            if got % 25 == 1 {
                println!(
                    "block {got}: {} packet(s), F={f}, DID {:02x} SDID {:02x}, units {}",
                    p.len(),
                    p.first().map_or(0, |a| a.did),
                    p.first().map_or(0, |a| a.sdid),
                    p.first().map_or(0, |a| a.udw[0] >> 4)
                );
            }
        }
    }
    let sent = sender.join().unwrap();
    let (packets, lost) = rx.stats();
    println!("sent {sent}, received {got} (F: {fields:?}), packets {packets}, lost {lost}");
    drop(rx);
    drop(tx);
    Ok(())
}
