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
    let v = Video { width: 1920, height: 1080, rate: (25, 1), interlaced: false };
    let leg = [Leg { port: 0, ip, udp: 20000 }];
    let rx = mtl.rx("loop-rx", &leg, 96, &v)?;
    let tx = mtl.tx("loop-tx", &leg, 96, &v)?;
    let start = Instant::now();
    let (mut sent, mut got) = (0u32, 0u32);
    while start.elapsed() < Duration::from_secs(6) {
        if let Some(f) = tx.next() {
            let n = sent as u8;
            f.data.fill(n);
            f.send_field(false);
            sent += 1;
        }
        while let Some(f) = rx.next() {
            if f.complete {
                got += 1;
                if got % 25 == 0 {
                    println!("received {got} frames, first byte {}", f.data[0]);
                }
            }
        }
    }
    let (packets, lost) = rx.stats();
    println!("sent {sent}, received {got}, packets {packets}, lost {lost}");
    if got == 0 {
        return Err("nothing received".into());
    }
    Ok(())
}
