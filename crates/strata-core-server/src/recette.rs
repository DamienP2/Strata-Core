//! `strata-core test` : essais de recette sans appli. Une mire et un bip sur une sortie, le niveau
//! du son d'une entrée. À lancer sous le compte du service (`sudo -u strata strata-core test …`) :
//! les flux MXL sont à lui.
use clap::Subcommand;
use std::time::{Duration, Instant};
use strata_mxl::{Mxl, Uuid, Video};

#[derive(Subcommand)]
pub enum TestCmd {
    /// Une mire (barres qui défilent, un éclair blanc chaque seconde) sur une sortie, avec un bip
    /// 1 kHz à -18 dBFS sur une sortie SDI (coupé pendant l'éclair : on voit le calage du son).
    /// La cible : `sdi/<carte>/<connecteur>` ou `2110/<flux>` (flux vidéo envoyé).
    Bars {
        target: String,
        /// Le format : 1080i50, 1080p25, 720p50, 1080i59.94, 2160p50…
        #[arg(long, default_value = "1080i50")]
        format: String,
        /// Sans le bip (SDI).
        #[arg(long)]
        no_tone: bool,
        /// Canaux du bip.
        #[arg(long, default_value_t = 2)]
        channels: u32,
    },
    /// Un bip 1 kHz à -18 dBFS seul : `sdi/<carte>/<connecteur>` ou `2110/<flux>` (flux de son envoyé).
    Tone {
        target: String,
        #[arg(long, default_value_t = 2)]
        channels: u32,
    },
    /// Le niveau du son d'une entrée, canal par canal, deux fois par seconde :
    /// `sdi/<carte>/<connecteur>`, `2110/<flux>` (flux de son reçu) ou l'identifiant MXL.
    Level {
        target: String,
        /// Combien de secondes (0 : jusqu'à Ctrl-C).
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
}

/// Les flux MXL d'une cible : (image, son).
fn flows(target: &str, output: bool) -> Result<(Uuid, Uuid), String> {
    if let Ok(id) = target.parse::<Uuid>() {
        return Ok((id, id));
    }
    let parts: Vec<&str> = target.split('/').collect();
    match parts.as_slice() {
        ["sdi", card, n] => {
            let base = if output { format!("sdi-out/{card}/{n}") } else { format!("sdi/{card}/{n}") };
            Ok((strata_mxl::flow_id(&base), strata_mxl::flow_id(&format!("{base}/audio"))))
        }
        ["2110", flow] => {
            let id = strata_mxl::flow_id(&format!("2110/{flow}"));
            Ok((id, id))
        }
        _ => Err(format!("« {target} » : attendu sdi/<carte>/<connecteur>, 2110/<flux> ou un identifiant MXL")),
    }
}

pub fn run(c: TestCmd) -> anyhow::Result<()> {
    let mxl = Mxl::open().map_err(|e| anyhow::anyhow!(e))?;
    match c {
        TestCmd::Bars { target, format, no_tone, channels } => {
            let (w, h, rate, interlaced) = crate::hardware::parse_format(&format).ok_or_else(|| anyhow::anyhow!("format inconnu : {format}"))?;
            let (vid, aid) = flows(&target, true).map_err(|e| anyhow::anyhow!(e))?;
            let tone = !no_tone && target.starts_with("sdi/");
            bars(&mxl, vid, tone.then_some((aid, channels.clamp(1, 16))), Video { width: w, height: h, rate, interlaced }).map_err(|e| anyhow::anyhow!(e))
        }
        TestCmd::Tone { target, channels } => {
            let (_, aid) = flows(&target, true).map_err(|e| anyhow::anyhow!(e))?;
            tone(&mxl, aid, channels.clamp(1, 16)).map_err(|e| anyhow::anyhow!(e))
        }
        TestCmd::Level { target, seconds } => {
            let (_, aid) = flows(&target, false).map_err(|e| anyhow::anyhow!(e))?;
            level(&mxl, aid, seconds).map_err(|e| anyhow::anyhow!(e))
        }
    }
}

/// -18 dBFS.
const AMP: f32 = 0.125_892_54;

fn sine(k: u64) -> f32 {
    AMP * (2.0 * std::f32::consts::PI * ((k % 48) as f32) / 48.0).sin()
}

fn bars(mxl: &Mxl, vid: Uuid, tone: Option<(Uuid, u32)>, v: Video) -> Result<(), String> {
    let mut w = mxl.video_writer(vid, "Mire (strata-core test)", &v)?;
    let mut aw = match tone {
        Some((aid, ch)) => Some((mxl.audio_writer(aid, "Bip (strata-core test)", ch)?, ch as usize)),
        None => None,
    };
    println!(
        "Mire {}x{}{} sur {vid}{}. Ctrl-C pour arrêter.",
        v.width,
        v.height,
        if v.interlaced { "i" } else { "p" },
        if aw.is_some() { " avec le bip" } else { "" }
    );
    // Huit barres à 75 % (blanc, jaune, cyan, vert, magenta, rouge, bleu, noir), en UYVY 8 bits.
    const BARS: [[u8; 3]; 8] = [[180, 128, 128], [168, 44, 136], [145, 147, 44], [133, 63, 52], [63, 193, 204], [51, 109, 212], [28, 212, 120], [16, 128, 128]];
    let (width, height) = (v.width as usize, v.height as usize);
    let stride = width * 2;
    // Deux largeurs d'une ligne : la ligne de l'image n en est une fenêtre décalée.
    let mut row = vec![0u8; stride * 2];
    for x in (0..width * 2).step_by(2) {
        let [y, cb, cr] = BARS[(x % width) * 8 / width];
        row[x * 2..x * 2 + 4].copy_from_slice(&[cb, y, cr, y]);
    }
    let white: Vec<u8> = [128u8, 235, 128, 235].repeat(width / 2 * height);
    let mut frame = vec![0u8; stride * height];
    let (num, den) = (v.rate.0.max(1) as u64, v.rate.1.max(1) as u64);
    let per_second = num.div_ceil(den);
    let start = Instant::now();
    let mut acc = 0u64;
    let mut phase = 0u64;
    for n in 0u64.. {
        let flash = n % per_second == 0;
        if flash {
            w.write_uyvy(&white, stride)?;
        } else {
            let off = ((n * 8) as usize % width) & !1;
            for y in 0..height {
                frame[y * stride..(y + 1) * stride].copy_from_slice(&row[off * 2..off * 2 + stride]);
            }
            w.write_uyvy(&frame, stride)?;
        }
        if let Some((a, ch)) = aw.as_mut() {
            // Le son de cette image, posé à son instant ; silence pendant l'éclair.
            acc += 48_000 * den;
            let k = (acc / num) as usize;
            acc %= num;
            let at = w.last_frame_audio_index().ok_or("pas d'instant pour l'image")?;
            let pcm: Vec<f32> = (0..k).flat_map(|i| std::iter::repeat_n(if flash { 0.0 } else { sine(phase + i as u64) }, *ch)).collect();
            phase += k as u64;
            a.write_at(at, &pcm)?;
        }
        let due = start + Duration::from_nanos((n + 1) * 1_000_000_000 * den / num);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    Ok(())
}

fn tone(mxl: &Mxl, aid: Uuid, ch: u32) -> Result<(), String> {
    let mut w = mxl.audio_writer(aid, "Bip (strata-core test)", ch)?;
    println!("Bip 1 kHz -18 dBFS, {ch} canaux, sur {aid}. Ctrl-C pour arrêter.");
    let start = Instant::now();
    let mut n = 0u64;
    loop {
        let pcm: Vec<f32> = (0..480).flat_map(|k| std::iter::repeat_n(sine(n + k), ch as usize)).collect();
        w.write_interleaved(&pcm)?;
        n += 480;
        let due = start + Duration::from_micros(n * 1_000_000 / 48_000);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}

fn level(mxl: &Mxl, aid: Uuid, seconds: u64) -> Result<(), String> {
    let r = mxl.audio_reader(aid).map_err(|e| format!("{e} (rien n'écrit ce flux ?)"))?;
    println!("Son de {aid} : {} canaux, niveau RMS sur 200 ms.", r.channels);
    let start = Instant::now();
    let mut buf = Vec::new();
    const N: usize = 9600;
    while seconds == 0 || start.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_millis(500));
        let line = match r.written() {
            Some(w) if w >= N as u64 && r.read_at(w + 1 - N as u64, N, &mut buf) => {
                let ch = r.channels.max(1);
                (0..ch)
                    .map(|c| {
                        let p = (0..N).map(|k| buf[k * ch + c].powi(2)).sum::<f32>() / N as f32;
                        let db = 10.0 * p.max(1e-12).log10();
                        if db < -90.0 {
                            format!("{:>2}: silence", c + 1)
                        } else {
                            format!("{:>2}: {db:>6.1}", c + 1)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("  ")
            }
            _ => "rien d'écrit".to_string(),
        };
        println!("{:>5.1} s  {line}", start.elapsed().as_secs_f32());
    }
    Ok(())
}
