//! Essai : un flux 1080p25 envoyé et reçu par le même port (socket du noyau).
//!
//! `STRATA_MTL_LIB=…/libmtl.so cargo run -p strata-mtl --example loopback -- <interface> <ip>`
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use strata_mtl::{load, Leg, Mtl, Pmd, Port, Video};

fn main() -> Result<(), String> {
    let mut a = std::env::args().skip(1);
    let iface = a.next().unwrap_or("lo".into());
    let ip: Ipv4Addr = a.next().unwrap_or("127.0.0.1".into()).parse().map_err(|e| format!("{e}"))?;
    println!("libmtl: {}", load()?.display());
    let port = Port { name: format!("kernel:{iface}"), pmd: Pmd::Kernel, ip, mask: Ipv4Addr::new(255, 0, 0, 0), gateway: Ipv4Addr::UNSPECIFIED };
    let mtl = Mtl::open(&[port], 37, None)?;
    // Facultatifs : l'adresse du flux (multicast) et 720p50 au lieu de 1080p25.
    let dst: Ipv4Addr = a.next().map(|s| s.parse().map_err(|e| format!("{e}"))).transpose()?.unwrap_or(ip);
    let v = if a.next().as_deref() == Some("720p50") {
        Video { width: 1280, height: 720, rate: (50, 1), interlaced: false }
    } else {
        Video { width: 1920, height: 1080, rate: (25, 1), interlaced: false }
    };
    let leg = [Leg { port: 0, ip: dst, udp: 20100, source: None }];
    let mut rx = mtl.rx("loop-rx", &leg, 96, &v)?;
    let mut tx = mtl.tx("loop-tx", &leg, 96, &v, strata_mtl::Pacing::default(), None)?;
    let start = Instant::now();
    let (mut sent, mut got) = (0u32, 0u32);
    while start.elapsed() < Duration::from_secs(6) {
        if let Some(f) = tx.frame() {
            let n = (sent as u8).max(1);
            f.data.fill(n);
            f.send_field(false);
            sent += 1;
        }
        while let Some(f) = rx.frame() {
            if f.complete {
                got += 1;
                if got % 25 == 0 {
                    let zero = f.data.iter().filter(|&&b| b == 0).count();
                    println!("received {got} frames, first byte {}, {zero} zero bytes of {}", f.data[0], f.data.len());
                }
            }
        }
    }
    let strata_mtl::RxStats { received: packets, lost, .. } = rx.stats();
    println!("sent {sent}, received {got}, packets {packets}, lost {lost}");
    if got == 0 {
        return Err("nothing received".into());
    }
    Ok(())
}
