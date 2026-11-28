//! La destination HTML5 : une page web, comme les sorties HTML5 de Strata Graphics, pour une
//! source navigateur (OBS, vMix, CasparCG…), à l'adresse choisie (`/output/<adresse>`).
//!
//! ffmpeg remet le TS de l'encodeur en MP4 fragmenté (H.264 + AAC, un fragment toutes les 250 ms
//! environ) ; le Core garde l'en-tête (`ftyp` + `moov`) et diffuse les fragments à chaque
//! page : `GET /output/<adresse>` (la page), `GET /live/<sortie>/stream.mp4` (le flux). Un
//! spectateur qui arrive reçoit l'en-tête puis part du prochain fragment qui commence par une
//! image clé ; un spectateur en retard saute jusqu'à la suivante. Latence : moins d'une
//! seconde plus la mise en mémoire du navigateur.
use axum::body::Bytes;
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Un fragment (`moof` + `mdat`), et s'il commence par une image clé.
#[derive(Clone)]
pub struct Frag {
    pub data: Bytes,
    pub key: bool,
    /// L'en-tête qui va avec : un nouvel en-tête (ffmpeg relancé) coupe les spectateurs.
    pub gen: u64,
}

#[derive(Default)]
struct Head {
    init: Option<Bytes>,
    codecs: String,
    gen: u64,
}

/// Le flux d'une sortie.
pub struct Channel {
    head: Mutex<Head>,
    tx: broadcast::Sender<Frag>,
}

impl Channel {
    fn new() -> Channel {
        Channel { head: Mutex::new(Head::default()), tx: broadcast::channel(256).0 }
    }

    /// L'en-tête, ses codecs (`avc1.64002a,mp4a.40.2`), son numéro ; et les fragments à venir.
    pub fn join(&self) -> Option<(Bytes, String, u64, broadcast::Receiver<Frag>)> {
        let rx = self.tx.subscribe();
        let h = self.head.lock().unwrap();
        h.init.clone().map(|i| (i, h.codecs.clone(), h.gen, rx))
    }

    /// Les codecs, quand le flux est là.
    pub fn codecs(&self) -> Option<String> {
        let h = self.head.lock().unwrap();
        h.init.as_ref().map(|_| h.codecs.clone())
    }

    /// Plus de flux (destination arrêtée, ffmpeg relancé).
    pub fn reset(&self) {
        let mut h = self.head.lock().unwrap();
        h.init = None;
        h.gen += 1;
    }

    fn set_init(&self, init: Vec<u8>) -> (u64, Option<u32>, HashMap<u32, u32>) {
        let info = parse_init(&init);
        let mut h = self.head.lock().unwrap();
        h.gen += 1;
        h.codecs = info.codecs;
        h.init = Some(Bytes::from(init));
        (h.gen, info.video, info.trex)
    }

    /// Lit la sortie de ffmpeg (MP4 fragmenté) jusqu'au bout.
    pub fn feed(&self, mut out: impl Read) {
        let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
        let mut chunk = vec![0u8; 64 * 1024];
        let mut ftyp: Vec<u8> = Vec::new();
        let mut moof: Vec<u8> = Vec::new();
        let (mut gen, mut video, mut trex) = (0u64, None, HashMap::new());
        loop {
            let n = match out.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            buf.extend_from_slice(&chunk[..n]);
            let mut at = 0usize;
            while let Some((kind, len)) = box_at(&buf[at..]) {
                if buf.len() - at < len {
                    break;
                }
                let b = &buf[at..at + len];
                match &kind {
                    b"ftyp" => ftyp = b.to_vec(),
                    b"moov" => {
                        let mut init = std::mem::take(&mut ftyp);
                        init.extend_from_slice(b);
                        (gen, video, trex) = self.set_init(init);
                    }
                    b"moof" => moof = b.to_vec(),
                    b"mdat" if !moof.is_empty() => {
                        let key = starts_with_key(&moof, video, &trex);
                        let mut data = std::mem::take(&mut moof);
                        data.extend_from_slice(b);
                        let _ = self.tx.send(Frag { data: Bytes::from(data), key, gen });
                    }
                    _ => {}
                }
                at += len;
            }
            if at > 0 {
                buf.drain(..at);
            }
            // Une boîte démesurée : le flux n'est pas ce qu'on attend, on repart de zéro.
            if buf.len() > 64 << 20 {
                buf.clear();
            }
        }
        self.reset();
    }
}

/// Les flux HTML5, par sortie.
#[derive(Default)]
pub struct Hubs {
    map: Mutex<HashMap<String, Arc<Channel>>>,
}

impl Hubs {
    /// Celui d'une sortie (créé au besoin).
    pub fn channel(&self, id: &str) -> Arc<Channel> {
        self.map.lock().unwrap().entry(id.to_string()).or_insert_with(|| Arc::new(Channel::new())).clone()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Channel>> {
        self.map.lock().unwrap().get(id).cloned()
    }
}

// ---------- MP4 ----------

/// Le type et la longueur de la boîte qui commence là (si son en-tête est complet).
fn box_at(b: &[u8]) -> Option<([u8; 4], usize)> {
    if b.len() < 8 {
        return None;
    }
    let size = u32::from_be_bytes(b[0..4].try_into().ok()?) as usize;
    let kind: [u8; 4] = b[4..8].try_into().ok()?;
    let len = match size {
        1 => {
            if b.len() < 16 {
                return None;
            }
            usize::try_from(u64::from_be_bytes(b[8..16].try_into().ok()?)).ok()?
        }
        // 0 (jusqu'à la fin) n'existe pas dans un flux sans fin : traité comme une boîte cassée.
        0..=7 => 8,
        n => n,
    };
    Some((kind, len))
}

/// Les boîtes filles d'un contenu : (type, contenu sans l'en-tête).
fn children(mut b: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let (kind, len) = box_at(b)?;
        let head = if u32::from_be_bytes(b[0..4].try_into().ok()?) == 1 { 16 } else { 8 };
        if len < head || len > b.len() {
            return None;
        }
        let body = &b[head..len];
        b = &b[len..];
        Some((kind, body))
    })
}

fn find<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(b).find(|(k, _)| k == kind).map(|(_, x)| x)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_be_bytes(x.try_into().unwrap()))
}

struct InitInfo {
    codecs: String,
    /// La piste vidéo.
    video: Option<u32>,
    /// Les drapeaux par défaut des échantillons, par piste (`trex`).
    trex: HashMap<u32, u32>,
}

fn parse_init(init: &[u8]) -> InitInfo {
    let mut out = InitInfo { codecs: String::new(), video: None, trex: HashMap::new() };
    let Some(moov) = children(init).find(|(k, _)| k == b"moov").map(|(_, x)| x) else { return out };
    let mut codecs = Vec::new();
    for (k, trak) in children(moov) {
        if &k != b"trak" {
            continue;
        }
        let id = find(trak, b"tkhd").and_then(|t| if t.first() == Some(&1) { u32_at(t, 20) } else { u32_at(t, 12) });
        let Some(mdia) = find(trak, b"mdia") else { continue };
        let handler = find(mdia, b"hdlr").and_then(|h| h.get(8..12));
        let stsd = find(mdia, b"minf").and_then(|m| find(m, b"stbl")).and_then(|s| find(s, b"stsd"));
        if handler == Some(b"vide") {
            out.video = id;
            // stsd : version et drapeaux, nombre d'entrées, puis l'entrée avc1 (78 octets avant ses boîtes).
            let avcc = stsd
                .and_then(|s| s.get(8..))
                .and_then(|e| children(e).find(|(k, _)| k == b"avc1").map(|(_, x)| x))
                .and_then(|a| a.get(78..))
                .and_then(|x| find(x, b"avcC"));
            match avcc {
                Some(c) if c.len() >= 4 => codecs.push(format!("avc1.{:02x}{:02x}{:02x}", c[1], c[2], c[3])),
                _ => codecs.push("avc1.640028".into()),
            }
        } else if handler == Some(b"soun") {
            codecs.push("mp4a.40.2".into());
        }
    }
    if let Some(mvex) = find(moov, b"mvex") {
        for (k, t) in children(mvex) {
            if &k == b"trex" {
                if let (Some(id), Some(flags)) = (u32_at(t, 4), u32_at(t, 20)) {
                    out.trex.insert(id, flags);
                }
            }
        }
    }
    out.codecs = codecs.join(",");
    out
}

/// Le fragment commence-t-il par une image clé (sur la piste vidéo) ?
fn starts_with_key(moof: &[u8], video: Option<u32>, trex: &HashMap<u32, u32>) -> bool {
    let Some(video) = video else { return true };
    let Some((_, body)) = children(moof).next().filter(|(k, _)| k == b"moof") else { return false };
    for (k, traf) in children(body) {
        if &k != b"traf" {
            continue;
        }
        let Some(tfhd) = find(traf, b"tfhd") else { continue };
        let (Some(tf), Some(track)) = (u32_at(tfhd, 0), u32_at(tfhd, 4)) else { continue };
        if track != video {
            continue;
        }
        // tfhd : les champs présents selon les drapeaux, dans l'ordre.
        let mut at = 8;
        for (bit, size) in [(0x1, 8), (0x2, 4), (0x8, 4), (0x10, 4)] {
            if tf & 0xff_ffff & bit != 0 {
                at += size;
            }
        }
        let mut flags = if tf & 0x20 != 0 { u32_at(tfhd, at) } else { trex.get(&track).copied() };
        if let Some(trun) = find(traf, b"trun") {
            let tr = u32_at(trun, 0).unwrap_or(0) & 0xff_ffff;
            let mut at = 8 + if tr & 0x1 != 0 { 4 } else { 0 };
            if tr & 0x4 != 0 {
                flags = u32_at(trun, at);
            } else if tr & 0x400 != 0 {
                // Les drapeaux du premier échantillon, après sa durée et sa taille s'il les a.
                at += if tr & 0x100 != 0 { 4 } else { 0 } + if tr & 0x200 != 0 { 4 } else { 0 };
                flags = u32_at(trun, at);
            }
        }
        // Image clé : « ne dépend d'aucune autre » (2) ou pas « non synchronisable ».
        return flags.is_some_and(|f| (f >> 24) & 3 == 2 || f & 0x1_0000 == 0);
    }
    false
}

/// Les options ffmpeg du MP4 fragmenté, écrit sur stdout.
pub fn ffmpeg_args() -> Vec<String> {
    ["-f", "mp4", "-movflags", "frag_keyframe+empty_moov+default_base_moof", "-frag_duration", "250000", "pipe:1"].map(String::from).to_vec()
}

/// La page d'une sortie HTML5, comme celles de Strata Graphics : sans rien autour (ni contrôles,
/// ni texte), transparente ou du fond choisi, pour une source navigateur (OBS, vMix, CasparCG…).
/// Le script est à part (la politique du Core refuse les scripts dans la page). `?scale=1` : à
/// la taille de l'image ; `?bg=<couleur>` : un autre fond, pour essayer ; `?debug` : la mesure.
pub fn output_page(title: &str, stream: &str, codecs: Option<&str>, background: &str) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mime = codecs.map(|c| format!("video/mp4; codecs=\"{c}\"")).unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="fr"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title>
<style>html,body{{margin:0;height:100%;overflow:hidden;background:transparent}}#o{{position:fixed;inset:0;overflow:hidden}}video{{position:absolute;inset:0;width:100%;height:100%;object-fit:contain}}video.one{{inset:auto;left:50%;top:50%;width:auto;height:auto;transform:translate(-50%,-50%)}}#d{{position:fixed;left:8px;top:8px;font:12px ui-monospace,monospace;color:#0f0;background:rgba(0,0,0,.6);padding:4px 6px;white-space:pre}}</style>
</head><body>
<div id="o" data-bg="{bg}"><video id="v" data-mime="{mime}" data-stream="{stream}" autoplay playsinline></video></div>
<script src="/live/output.js"></script>
</body></html>
"#,
        title = esc(title),
        mime = esc(&mime),
        stream = esc(stream),
        bg = esc(background),
    )
}

/// La politique de la page : elle peut être mise dans la page d'un autre site.
pub const OUTPUT_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; media-src 'self' blob:; connect-src 'self'; frame-ancestors *";

/// La sortie : MediaSource (ManagedMediaSource sur iPhone). Elle reste à environ 0,6 s du direct
/// en jouant à peine plus vite ou plus lentement (jamais par sauts, l'image reste fluide) ; elle
/// ne saute qu'après un vrai trou (plus de 4 s de retard). Rien ne s'affiche tant que le flux
/// n'est pas là (une source navigateur est à l'antenne). Le son : joué si la page le peut (source
/// navigateur), sinon coupé.
pub const OUTPUT_JS: &str = r#"'use strict';
const o = document.getElementById('o'), v = document.getElementById('v');
const mime = v.dataset.mime, stream = v.dataset.stream, q = new URLSearchParams(location.search);
o.style.background = q.get('bg') || o.dataset.bg || 'transparent';
if (q.get('scale') === '1') v.className = 'one';
const MS = window.ManagedMediaSource || window.MediaSource;
const TARGET = 0.6;
let dbg = null, frames = 0, last = performance.now();
if (q.has('debug')) { dbg = document.createElement('div'); dbg.id = 'd'; document.body.append(dbg); }
function note(text) { if (dbg) dbg.textContent = text; }
function again() { setTimeout(() => location.reload(), 3000); }
function start() {
  v.play().catch(() => { v.muted = true; v.play().catch(() => {}); });
}
function steer(sb) {
  const b = sb.buffered;
  if (!b.length) return;
  const s = b.start(0), end = b.end(b.length - 1), lag = end - v.currentTime;
  if (v.currentTime < s || lag > 4) { v.currentTime = Math.max(s, end - TARGET); v.playbackRate = 1; }
  else v.playbackRate = lag > TARGET + 0.6 ? 1.05 : lag > TARGET + 0.25 ? 1.02 : lag < TARGET - 0.3 ? 0.97 : 1;
  if (v.paused && lag > 0.2) start();
  if (dbg) { const q2 = v.getVideoPlaybackQuality ? v.getVideoPlaybackQuality() : null; note(`retard ${lag.toFixed(2)} s · vitesse ${v.playbackRate}` + (q2 ? ` · images ${q2.totalVideoFrames} · perdues ${q2.droppedVideoFrames}` : '') + (v.muted ? ' · son coupé' : '')); }
}
async function play() {
  if (!mime) { note('en attente du flux'); return again(); }
  if (!MS || !MS.isTypeSupported(mime)) { v.src = stream; start(); return; }
  const ms = new MS();
  v.disableRemotePlayback = true;
  v.src = URL.createObjectURL(ms);
  await new Promise((r) => ms.addEventListener('sourceopen', r, { once: true }));
  const sb = ms.addSourceBuffer(mime);
  const queue = [];
  const pump = () => {
    if (sb.updating || !queue.length) return;
    try { sb.appendBuffer(queue.shift()); } catch (e) { queue.length = 0; }
  };
  sb.addEventListener('updateend', () => {
    const b = sb.buffered;
    if (b.length && v.currentTime - b.start(0) > 30) { sb.remove(b.start(0), v.currentTime - 10); return; }
    steer(sb);
    pump();
  });
  const res = await fetch(stream, { cache: 'no-store' });
  if (!res.ok || !res.body) { note('flux indisponible'); return again(); }
  const reader = res.body.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    queue.push(value);
    pump();
  }
  note('flux interrompu');
  again();
}
play().catch(() => again());
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn moof(track: u32, first_flags: u32) -> Vec<u8> {
        let mut tfhd = vec![0, 0x02, 0, 0]; // default-base-is-moof
        tfhd.extend_from_slice(&track.to_be_bytes());
        let mut trun = vec![0, 0, 0x02, 0x05]; // data offset, first sample flags, sample size
        trun.extend_from_slice(&1u32.to_be_bytes());
        trun.extend_from_slice(&0i32.to_be_bytes());
        trun.extend_from_slice(&first_flags.to_be_bytes());
        trun.extend_from_slice(&100u32.to_be_bytes());
        let traf = [bx(b"tfhd", &tfhd), bx(b"trun", &trun)].concat();
        bx(b"moof", &[bx(b"mfhd", &[0; 8]), bx(b"traf", &traf)].concat())
    }

    #[test]
    fn key_fragments_are_found() {
        let trex = HashMap::new();
        assert!(starts_with_key(&moof(1, 0x0200_0000), Some(1), &trex));
        assert!(!starts_with_key(&moof(1, 0x0101_0000), Some(1), &trex));
        // Une autre piste (le son) ne compte pas.
        assert!(!starts_with_key(&moof(2, 0x0200_0000), Some(1), &trex));
    }

    #[test]
    fn the_stream_is_split_into_fragments() {
        let ch = Channel::new();
        let mut rx = ch.tx.subscribe();
        let mut tkhd = vec![0u8; 24];
        tkhd[12..16].copy_from_slice(&1u32.to_be_bytes());
        let mut hdlr = vec![0u8; 12];
        hdlr[8..12].copy_from_slice(b"vide");
        let mut avc1 = vec![0u8; 78];
        avc1.extend(bx(b"avcC", &[1, 0x64, 0, 0x2a]));
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend(bx(b"avc1", &avc1));
        let stbl = bx(b"stbl", &bx(b"stsd", &stsd));
        let mdia = bx(b"mdia", &[bx(b"hdlr", &hdlr), bx(b"minf", &stbl)].concat());
        let moov = bx(b"moov", &bx(b"trak", &[bx(b"tkhd", &tkhd), mdia].concat()));
        let stream = [bx(b"ftyp", b"isom"), moov, moof(1, 0x0200_0000), bx(b"mdat", &[7; 100]), moof(1, 0x0101_0000), bx(b"mdat", &[8; 100])].concat();
        // Lu par petits morceaux, comme d'un tube.
        struct Slow<'a>(&'a [u8]);
        impl Read for Slow<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.len().min(out.len()).min(13);
                out[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        ch.feed(Slow(&stream));
        let a = rx.try_recv().unwrap();
        let b = rx.try_recv().unwrap();
        assert!(a.key && !b.key);
        assert_eq!(a.data.len(), moof(1, 0).len() + 108);
        assert_eq!(a.gen, b.gen);
        // Le flux fini : plus d'en-tête.
        assert!(ch.codecs().is_none());
    }

    #[test]
    fn codecs_come_from_the_header() {
        let mut tkhd = vec![0u8; 24];
        tkhd[12..16].copy_from_slice(&1u32.to_be_bytes());
        let mut hdlr = vec![0u8; 12];
        hdlr[8..12].copy_from_slice(b"vide");
        let mut avc1 = vec![0u8; 78];
        avc1.extend(bx(b"avcC", &[1, 0x64, 0, 0x2a]));
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend(bx(b"avc1", &avc1));
        let mdia = bx(b"mdia", &[bx(b"hdlr", &hdlr), bx(b"minf", &bx(b"stbl", &bx(b"stsd", &stsd)))].concat());
        let mut shdlr = vec![0u8; 12];
        shdlr[8..12].copy_from_slice(b"soun");
        let audio = bx(b"trak", &bx(b"mdia", &bx(b"hdlr", &shdlr)));
        let init = [bx(b"ftyp", b"isom"), bx(b"moov", &[bx(b"trak", &[bx(b"tkhd", &tkhd), mdia].concat()), audio].concat())].concat();
        let i = parse_init(&init);
        assert_eq!(i.codecs, "avc1.64002a,mp4a.40.2");
        assert_eq!(i.video, Some(1));
    }

    /// Sur une vraie sortie de ffmpeg (sauté sans ffmpeg) : en-tête, codecs, images clés.
    #[test]
    fn real_ffmpeg_output() {
        // Comme en vrai : l'encodeur fait du TS, la destination le recopie en MP4 fragmenté.
        let ts = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "4",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "50",
            "-keyint_min",
            "50",
            "-c:a",
            "aac",
            "-f",
            "mpegts",
            "pipe:1",
        ];
        let Ok(ts) = std::process::Command::new("ffmpeg").args(ts).output() else { return };
        if !ts.status.success() {
            return;
        }
        let d = super::super::Destination { url: "html5".into(), ..Default::default() };
        let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error"].map(String::from).to_vec();
        args.extend(super::super::engine::destination_args(&d, std::path::Path::new("/nonexistent")));
        use std::io::Write;
        let mut p = std::process::Command::new("ffmpeg").args(&args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut stdin = p.stdin.take().unwrap();
        let feed = std::thread::spawn(move || {
            let _ = stdin.write_all(&ts.stdout);
        });
        let out = p.wait_with_output().unwrap();
        feed.join().unwrap();
        assert!(out.status.success());
        let ch = Channel::new();
        let mut rx = ch.tx.subscribe();
        let info = parse_init(&out.stdout);
        assert!(info.codecs.starts_with("avc1.") && info.codecs.ends_with(",mp4a.40.2"), "{}", info.codecs);
        ch.feed(&out.stdout[..]);
        let mut frags = Vec::new();
        while let Ok(f) = rx.try_recv() {
            frags.push(f.key);
        }
        // 4 s à 250 ms : une quinzaine de fragments, une image clé toutes les 2 s.
        assert!(frags.len() >= 8, "{frags:?}");
        assert!(frags[0], "{frags:?}");
        let keys = frags.iter().filter(|k| **k).count();
        assert!((2..=3).contains(&keys), "{frags:?}");
    }
}
