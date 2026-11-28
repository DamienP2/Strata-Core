//! `strata-core test` : essais de recette sans appli. Une mire et un bip sur une sortie, le niveau
//! du son d'une entrée. À lancer sous le compte du service (`sudo -u strata strata-core test …`) :
//! les flux MXL sont à lui.
use clap::Subcommand;
use std::time::{Duration, Instant};
use strata_mxl::{Mxl, Uuid, Video};

#[derive(Subcommand)]
pub enum TestCmd {
    /// Une mire (barres qui défilent, un éclair blanc chaque seconde) sur une sortie, avec un bip
    /// 1 kHz à -18 dBFS sur une sortie SDI ou de flux réseau (coupé pendant l'éclair : on voit le
    /// calage du son). La cible : `sdi/<carte>/<connecteur>`, `2110/<flux>` (flux vidéo envoyé)
    /// `stream/<sortie>` (sortie de flux réseau) ou `ndi/<destination>` (destination NDI).
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
        /// Le flux de son où poser le bip, calé sur l'image : `2110/<flux de son envoyé>` (ST 2110
        /// envoie l'image et le son dans deux flux).
        #[arg(long)]
        audio: Option<String>,
    },
    /// Le calage image/son de ce qui est reçu, mesuré comme une appli le lit : la mire de `bars`
    /// (éclair et bip coupé) dans un flux vidéo et un flux de son, `2110/<flux>` ou les
    /// identifiants MXL. Positif : le son est en retard sur l'image.
    Sync {
        video: String,
        audio: String,
        /// Combien de secondes.
        #[arg(long, default_value_t = 20)]
        seconds: u64,
    },
    /// Un bip 1 kHz à -18 dBFS seul : `sdi/<carte>/<connecteur>` ou `2110/<flux>` (flux de son envoyé).
    Tone {
        target: String,
        #[arg(long, default_value_t = 2)]
        channels: u32,
    },
    /// Un timecode (ATC, ST 12-2, l'heure du serveur) dans un flux de données 2110-40 envoyé :
    /// `2110/<flux>`.
    Timecode {
        target: String,
        /// La cadence vidéo du flux : 1080i50, 1080p25…
        #[arg(long, default_value = "1080i50")]
        format: String,
    },
    /// Les paquets de données (ANC) reçus, une ligne par seconde : `2110/<flux>` (flux de données
    /// reçu) ou l'identifiant MXL.
    Data {
        target: String,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
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
        // Une sortie de flux réseau (celle qu'une appli alimente).
        ["stream", out] if output => {
            let base = crate::streams::output_flow(out);
            Ok((strata_mxl::flow_id(&base), strata_mxl::flow_id(&crate::streams::audio_of(&base))))
        }
        // Une destination NDI (celle qu'une appli alimente).
        ["ndi", out] if output => {
            let base = crate::hardware::ndi_out::out_flow(out);
            Ok((strata_mxl::flow_id(&base), strata_mxl::flow_id(&crate::streams::audio_of(&base))))
        }
        _ => Err(format!("« {target} » : attendu sdi/<carte>/<connecteur>, 2110/<flux>, stream/<sortie>, ndi/<destination> ou un identifiant MXL")),
    }
}

pub fn run(c: TestCmd) -> anyhow::Result<()> {
    let mxl = Mxl::open().map_err(|e| anyhow::anyhow!(e))?;
    match c {
        TestCmd::Bars { target, format, no_tone, channels, audio } => {
            let (w, h, rate, interlaced) = crate::hardware::parse_format(&format).ok_or_else(|| anyhow::anyhow!("format inconnu : {format}"))?;
            let (vid, mut aid) = flows(&target, true).map_err(|e| anyhow::anyhow!(e))?;
            if let Some(a) = &audio {
                aid = flows(a, true).map_err(|e| anyhow::anyhow!(e))?.1;
            }
            let tone = !no_tone && (audio.is_some() || ["sdi/", "stream/", "ndi/"].iter().any(|p| target.starts_with(p)));
            bars(&mxl, vid, tone.then_some((aid, channels.clamp(1, 16))), Video { width: w, height: h, rate, interlaced }).map_err(|e| anyhow::anyhow!(e))
        }
        TestCmd::Sync { video, audio, seconds } => {
            let (vid, _) = flows(&video, false).map_err(|e| anyhow::anyhow!(e))?;
            let (_, aid) = flows(&audio, false).map_err(|e| anyhow::anyhow!(e))?;
            sync(&mxl, vid, aid, seconds).map_err(|e| anyhow::anyhow!(e))
        }
        TestCmd::Tone { target, channels } => {
            let (_, aid) = flows(&target, true).map_err(|e| anyhow::anyhow!(e))?;
            tone(&mxl, aid, channels.clamp(1, 16)).map_err(|e| anyhow::anyhow!(e))
        }
        TestCmd::Timecode { target, format } => {
            let (_, _, rate, interlaced) = crate::hardware::parse_format(&format).ok_or_else(|| anyhow::anyhow!("format inconnu : {format}"))?;
            let (id, _) = flows(&target, true).map_err(|e| anyhow::anyhow!(e))?;
            timecode(&mxl, id, rate, interlaced).map_err(|e| anyhow::anyhow!(e))
        }
        TestCmd::Data { target, seconds } => {
            let (id, _) = flows(&target, false).map_err(|e| anyhow::anyhow!(e))?;
            data(&mxl, id, seconds).map_err(|e| anyhow::anyhow!(e))
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

/// Le début du trou de silence (24 échantillons nuls de suite après du son) dans `pcm` (entrelacé,
/// `ch` canaux, premier canal), en échantillons depuis le début.
fn silence_onset(pcm: &[f32], ch: usize) -> Option<usize> {
    let (mut run, mut start, mut tone) = (0usize, 0usize, false);
    for (k, x) in pcm.iter().step_by(ch.max(1)).enumerate() {
        if x.abs() < 1e-6 {
            if run == 0 {
                start = k;
            }
            run += 1;
            if run == 24 && tone {
                return Some(start);
            }
        } else {
            run = 0;
            tone = true;
        }
    }
    None
}

fn sync(mxl: &Mxl, vid: Uuid, aid: Uuid, seconds: u64) -> Result<(), String> {
    let r = mxl.video_reader(vid).map_err(|e| format!("{e} (rien n'écrit le flux vidéo ?)"))?;
    let a = mxl.audio_reader(aid).map_err(|e| format!("{e} (rien n'écrit le flux de son ?)"))?;
    let v = r.video;
    let period = Duration::from_secs_f64(v.rate.1.max(1) as f64 / v.rate.0.max(1) as f64);
    println!("Calage de {vid} (image, {}x{}{}) et {aid} (son), {seconds} s.", v.width, v.height, if v.interlaced { "i" } else { "p" });
    let stride = strata_mxl::v210_stride(v.width);
    let row = v.grain_lines() as usize / 2;
    let mut line = vec![0u8; v.width as usize * 2];
    // L'image et le son se lisent comme une appli : l'instant de l'image (en échantillons) puis le
    // son à cet instant. ±50 ms autour (MXL garde peu de son).
    const SPAN: u64 = 2_400;
    let mut pending: std::collections::VecDeque<u64> = Default::default();
    let (mut last, mut was_flash) = (None, false);
    let mut offsets: Vec<f64> = Vec::new();
    let mut buf = Vec::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(period / 4);
        let luma = r.with_latest(period * 3, |f, _| {
            strata_mxl::v210_to_uyvy(&f[row * stride..(row + 1) * stride], &mut line, v.width as usize);
            line[v.width as usize / 2 * 2 + 1]
        });
        let at = r.last_frame_audio_index();
        if let (Some(luma), Some(at)) = (luma, at) {
            if last != Some(at) {
                last = Some(at);
                let flash = luma > 200;
                if flash && !was_flash {
                    pending.push_back(at);
                }
                was_flash = flash;
            }
        }
        while let Some(&at) = pending.front() {
            let Some(written) = a.written() else { break };
            if written < at + SPAN {
                break;
            }
            pending.pop_front();
            // Par morceaux de 50 ms (MXL limite une lecture).
            let mut ok = at >= SPAN;
            buf.clear();
            let mut part = Vec::new();
            for k in (0..2 * SPAN).step_by(2_400) {
                ok = ok && a.read_at(at - SPAN + k, 2_400, &mut part);
                buf.extend_from_slice(&part);
            }
            if !ok {
                println!("éclair à {at} : son plus là");
                continue;
            }
            match silence_onset(&buf, a.channels) {
                Some(k) => {
                    let ms = (k as f64 - SPAN as f64) / 48.0;
                    offsets.push(ms);
                    println!("éclair à {at} : silence à {:+.2} ms", ms);
                }
                None => println!("éclair à {at} : pas de silence à ±50 ms"),
            }
        }
    }
    if offsets.is_empty() {
        return Err("aucune mesure (la mire et son bip arrivent-ils ?)".into());
    }
    let mean = offsets.iter().sum::<f64>() / offsets.len() as f64;
    let (lo, hi) = offsets.iter().fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(*x), h.max(*x)));
    println!(
        "{} mesures : moyenne {mean:+.2} ms, de {lo:+.2} à {hi:+.2} ms (positif : son en retard ; une image = {:.1} ms)",
        offsets.len(),
        period.as_secs_f64() * 1000.0
    );
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

/// Le timecode de l'heure du serveur (heure locale), en images.
fn wall_timecode(rate: (u32, u32)) -> (u8, u8, u8, u8) {
    let now = jiff::Timestamp::now().to_zoned_datetime(jiff::tz::TimeZone::system());
    let fps = (rate.0 as f64 / rate.1.max(1) as f64).round().max(1.0) as u64;
    let frame = (now.subsec_nanosecond().max(0) as u64 * fps / 1_000_000_000) as u8;
    (now.hour() as u8, now.minute() as u8, now.second() as u8, frame)
}

fn timecode(mxl: &Mxl, id: Uuid, rate: (u32, u32), interlaced: bool) -> Result<(), String> {
    let grains = if interlaced { (rate.0 * 2, rate.1) } else { rate };
    let mut w = mxl.data_writer(id, "Timecode (strata-core test)", grains)?;
    println!("Timecode ATC (heure du serveur) sur {id}. Ctrl-C pour arrêter.");
    let (num, den) = (grains.0.max(1) as u64, grains.1.max(1) as u64);
    let start = Instant::now();
    for n in 0u64.. {
        let (h, m, s, f) = wall_timecode(rate);
        let field = if interlaced { 2 + (n % 2) as u8 } else { 0 };
        // En entrelacé, le timecode dans la première trame seulement.
        let packets = if field == 3 { vec![] } else { vec![strata_mxl::anc::atc_packet(h, m, s, f, false)] };
        w.write(&packets, field)?;
        if n % num.div_ceil(den) == 0 {
            println!("{h:02}:{m:02}:{s:02}:{f:02}");
        }
        let due = start + Duration::from_nanos((n + 1) * 1_000_000_000 * den / num);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    Ok(())
}

fn data(mxl: &Mxl, id: Uuid, seconds: u64) -> Result<(), String> {
    let mut r = mxl.data_reader(id).map_err(|e| format!("{e} (rien n'écrit ce flux ?)"))?;
    let (num, den) = r.rate();
    println!("Données de {id} : {:.2} grains/s.", num as f64 / den.max(1) as f64);
    let start = Instant::now();
    let mut tick = Instant::now();
    let (mut grains, mut packets) = (0u64, std::collections::BTreeMap::<(u8, u8), u64>::new());
    let mut tc = None;
    while seconds == 0 || start.elapsed() < Duration::from_secs(seconds) {
        match r.next_grain() {
            Some((ps, _)) => {
                grains += 1;
                for p in &ps {
                    *packets.entry((p.did, p.sdid)).or_default() += 1;
                    if let Some(t) = strata_mxl::anc::atc_timecode(p) {
                        tc = Some(t);
                    }
                }
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
        if tick.elapsed() >= Duration::from_secs(1) {
            tick = Instant::now();
            let list: Vec<String> = packets.iter().map(|((d, s), n)| format!("{d:02X}/{s:02X} ×{n}")).collect();
            let tc = tc.map(|(h, m, s, f, _)| format!("  TC {h:02}:{m:02}:{s:02}:{f:02}")).unwrap_or_default();
            println!(
                "{:>5.1} s  {grains} grains  {}{tc}",
                start.elapsed().as_secs_f32(),
                if list.is_empty() { "aucun paquet".into() } else { list.join("  ") }
            );
            grains = 0;
            packets.clear();
        }
    }
    Ok(())
}
