//! Essai : un flux JPEG XS (ST 2110-22) 1080p25 envoyé et reçu par le même port (socket du noyau),
//! une mire en dégradé ; l'écart moyen après compression est affiché.
//!
//! Le greffon SVT-JPEG-XS doit être à côté de libmtl.
//! `STRATA_MTL_LIB=…/libmtl.so cargo run -p strata-mtl --example jxs_loopback -- [bpp]`
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use strata_mtl::{load, register_plugins, Leg, Mtl, Pmd, Port, Video};

fn main() -> Result<(), String> {
    let bpp: f32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(4.0);
    let lib = load()?;
    println!("libmtl: {}", lib.display());
    let plugin = register_plugins(&lib, &std::env::temp_dir()).ok_or("no JPEG XS plugin next to libmtl")?;
    println!("plugin: {}", plugin.display());
    let ip = Ipv4Addr::LOCALHOST;
    let port = Port { name: "kernel:lo".into(), pmd: Pmd::Kernel, ip, mask: Ipv4Addr::new(255, 0, 0, 0), gateway: Ipv4Addr::UNSPECIFIED };
    let mtl = Mtl::open(&[port], 37, None)?;
    let v = Video { width: 1920, height: 1080, rate: (25, 1), interlaced: false };
    let leg = [Leg { port: 0, ip, udp: 20020, source: None }];
    let mut rx = mtl.rx22("jxs-rx", &leg, 112, &v)?;
    let mut tx = mtl.tx22("jxs-tx", &leg, 112, &v, bpp, None)?;
    // La mire : Y en dégradé horizontal, chroma neutre.
    let stride = (1920usize).div_ceil(48) * 128;
    let mut pattern = vec![0u8; stride * 1080];
    for row in 0..1080 {
        let line = &mut pattern[row * stride..(row + 1) * stride];
        for p in 0..960 {
            let y0 = 64 + (2 * p * 876 / 1920) as u32;
            let s = [512u32, y0, 512, y0 + 1];
            for (j, val) in s.iter().enumerate() {
                let k = 4 * p + j;
                let at = 4 * (k / 3);
                let mut w = u32::from_le_bytes(line[at..at + 4].try_into().unwrap());
                w |= (val & 0x3ff) << (10 * (k % 3));
                line[at..at + 4].copy_from_slice(&w.to_le_bytes());
            }
        }
    }
    let start = Instant::now();
    let (mut sent, mut got, mut diff) = (0u32, 0u32, 0f64);
    while start.elapsed() < Duration::from_secs(8) {
        if let Some(f) = tx.frame() {
            for row in 0..f.height as usize {
                f.data[row * f.linesize..row * f.linesize + stride].copy_from_slice(&pattern[row * stride..(row + 1) * stride]);
            }
            f.send_field(false);
            sent += 1;
        }
        if let Some(f) = rx.frame() {
            if f.complete {
                got += 1;
                // L'écart moyen sur une ligne du milieu (octets v210 : grossier mais parlant).
                let row = 540 * f.linesize;
                let a = &f.data[row..row + stride];
                let b = &pattern[540 * stride..541 * stride];
                diff = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / stride as f64;
                if got % 25 == 0 {
                    println!("received {got} frames, mean byte gap on a line {diff:.2}");
                }
            }
        }
    }
    println!("sent {sent}, received {got}, last gap {diff:.2}");
    if got == 0 {
        return Err("nothing received".into());
    }
    Ok(())
}
