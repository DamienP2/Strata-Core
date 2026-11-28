//! Les fichiers SDP des flux ST 2110 : faits pour nos émetteurs (le `transportfile` d'IS-05),
//! lus quand un contrôleur branche un de nos récepteurs (`transport_file` du PATCH).
use std::net::Ipv4Addr;

/// Une vidéo : largeur, hauteur, images par seconde (n/d), entrelacée.
pub type VideoFormat = (u32, u32, (u32, u32), bool);

/// Une patte d'un flux (une par réseau en 2022-7).
#[derive(Debug, Clone, PartialEq)]
pub struct SdpLeg {
    pub group: Ipv4Addr,
    pub port: u16,
    /// L'adresse de l'émetteur (`a=source-filter`), ou celle d'origine (`o=`).
    pub source: Option<Ipv4Addr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Media {
    Video(VideoFormat),
    /// Canaux, bits (24 ou 16), durée d'un paquet en µs.
    Audio {
        channels: u16,
        bits: u8,
        ptime_us: u32,
    },
    /// Données annexes (ST 2110-40) : la cadence vidéo, si le SDP la donne, et entrelacée.
    Data {
        rate: Option<(u32, u32)>,
        interlaced: bool,
    },
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sdp {
    pub legs: Vec<SdpLeg>,
    pub media: Media,
    pub payload_type: u8,
}

/// Ce qu'il faut pour écrire le SDP d'un émetteur.
pub struct SenderSdp<'a> {
    pub name: &'a str,
    pub session: u64,
    pub version: u64,
    /// Par patte : le groupe, le port, l'adresse du port média qui envoie.
    pub legs: Vec<(Ipv4Addr, u16, Ipv4Addr)>,
    pub media: Media,
    pub payload_type: u8,
    /// `a=ts-refclk` : `ptp=IEEE1588-2008:…` ou `localmac=…`.
    pub refclk: String,
}

/// « 25 », « 30000/1001 ».
fn rate_text((n, d): (u32, u32)) -> String {
    if d == 1 {
        n.to_string()
    } else {
        format!("{n}/{d}")
    }
}

/// L'ordre des canaux (ST 2110-30) : stéréo, mono, ou « indéfini » pour les autres.
fn channel_order(n: u16) -> String {
    match n {
        1 => "SMPTE2110.(M)".into(),
        2 => "SMPTE2110.(ST)".into(),
        n => format!("SMPTE2110.(U{n:02})"),
    }
}

pub fn make(s: &SenderSdp) -> String {
    let origin = s.legs.first().map(|l| l.2).unwrap_or(Ipv4Addr::UNSPECIFIED);
    let mut out = format!("v=0\r\no=- {} {} IN IP4 {origin}\r\ns={}\r\nt=0 0\r\n", s.session, s.version, s.name.replace(['\r', '\n'], " "));
    let two = s.legs.len() == 2;
    if two {
        out.push_str("a=group:DUP primary secondary\r\n");
    }
    let pt = s.payload_type;
    for (i, (group, port, source)) in s.legs.iter().enumerate() {
        match &s.media {
            Media::Video((w, h, rate, interlaced)) => {
                out.push_str(&format!("m=video {port} RTP/AVP {pt}\r\nc=IN IP4 {group}/64\r\na=source-filter: incl IN IP4 {group} {source}\r\n"));
                out.push_str(&format!("a=rtpmap:{pt} raw/90000\r\n"));
                out.push_str(&format!(
                    "a=fmtp:{pt} sampling=YCbCr-4:2:2; width={w}; height={h}; exactframerate={}; depth=10; TCS=SDR; colorimetry=BT709; PM=2110GPM; SSN=ST2110-20:2017; TP=2110TPN;{}\r\n",
                    rate_text(*rate),
                    if *interlaced { " interlace;" } else { "" }
                ));
            }
            Media::Audio { channels, bits, ptime_us } => {
                out.push_str(&format!("m=audio {port} RTP/AVP {pt}\r\nc=IN IP4 {group}/64\r\na=source-filter: incl IN IP4 {group} {source}\r\n"));
                out.push_str(&format!("a=rtpmap:{pt} L{bits}/48000/{channels}\r\n"));
                out.push_str(&format!("a=fmtp:{pt} channel-order={}\r\n", channel_order(*channels)));
                let ms = *ptime_us as f64 / 1000.0;
                out.push_str(&format!("a=ptime:{ms}\r\na=maxptime:{ms}\r\n"));
            }
            Media::Data { rate, interlaced } => {
                out.push_str(&format!("m=video {port} RTP/AVP {pt}\r\nc=IN IP4 {group}/64\r\na=source-filter: incl IN IP4 {group} {source}\r\n"));
                out.push_str(&format!("a=rtpmap:{pt} smpte291/90000\r\n"));
                let mut fmtp: Vec<String> = Vec::new();
                if let Some(r) = rate {
                    fmtp.push(format!("exactframerate={}", rate_text(*r)));
                }
                if *interlaced {
                    fmtp.push("interlace".into());
                }
                if !fmtp.is_empty() {
                    out.push_str(&format!("a=fmtp:{pt} {}\r\n", fmtp.join("; ")));
                }
            }
            Media::Other(_) => {}
        }
        out.push_str(&format!("a=ts-refclk:{}\r\na=mediaclk:direct=0\r\n", s.refclk));
        if two {
            out.push_str(if i == 0 { "a=mid:primary\r\n" } else { "a=mid:secondary\r\n" });
        }
    }
    out
}

/// Lit le SDP d'un émetteur : ses pattes, et ce qu'il envoie.
pub fn parse(text: &str) -> Result<Sdp, String> {
    let mut origin: Option<Ipv4Addr> = None;
    let mut session_c: Option<Ipv4Addr> = None;
    // Par section m= : (genre, port, pt, c=, source-filter, rtpmap, fmtp, ptime)
    struct M {
        kind: String,
        port: u16,
        pt: u8,
        c: Option<Ipv4Addr>,
        source: Option<Ipv4Addr>,
        rtpmap: Option<String>,
        fmtp: Option<String>,
        ptime: Option<f64>,
    }
    let mut ms: Vec<M> = Vec::new();
    let ip = |s: &str| s.split('/').next().and_then(|a| a.trim().parse::<Ipv4Addr>().ok());
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "o" => origin = v.split_whitespace().nth(5).and_then(ip),
            "m" => {
                let p: Vec<&str> = v.split_whitespace().collect();
                if p.len() < 4 {
                    return Err(format!("bad SDP line m={v}"));
                }
                ms.push(M {
                    kind: p[0].to_string(),
                    port: p[1].split('/').next().and_then(|x| x.parse().ok()).ok_or(format!("bad SDP port in m={v}"))?,
                    pt: p[3].parse().map_err(|_| format!("bad SDP payload type in m={v}"))?,
                    c: None,
                    source: None,
                    rtpmap: None,
                    fmtp: None,
                    ptime: None,
                })
            }
            "c" => {
                let a = v.split_whitespace().nth(2).and_then(ip);
                match ms.last_mut() {
                    Some(m) => m.c = a,
                    None => session_c = a,
                }
            }
            "a" => {
                let Some(m) = ms.last_mut() else { continue };
                let (name, value) = v.split_once(':').unwrap_or((v, ""));
                match name {
                    // a=source-filter: incl IN IP4 <groupe> <source>
                    "source-filter" => m.source = value.split_whitespace().nth(4).and_then(ip),
                    "rtpmap" => m.rtpmap = value.split_once(' ').map(|(_, r)| r.trim().to_string()),
                    "fmtp" => m.fmtp = value.split_once(' ').map(|(_, r)| r.trim().to_string()),
                    "ptime" => m.ptime = value.trim().parse().ok(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    let first = ms.first().ok_or("the SDP has no media (m=)")?;
    let fmtp = |m: &M, key: &str| -> Option<String> {
        m.fmtp.as_ref()?.split(';').map(str::trim).find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            k.trim().eq_ignore_ascii_case(key).then(|| v.trim().to_string())
        })
    };
    let rtpmap = first.rtpmap.clone().unwrap_or_default();
    let encoding = rtpmap.split('/').next().unwrap_or("").to_ascii_lowercase();
    let media = match (first.kind.as_str(), encoding.as_str()) {
        ("video", "raw") => {
            let num = |k: &str| fmtp(first, k).and_then(|v| v.parse::<u32>().ok()).ok_or(format!("the SDP has no {k}"));
            let rate = fmtp(first, "exactframerate").ok_or("the SDP has no exactframerate")?;
            let rate = match rate.split_once('/') {
                Some((n, d)) => (n.parse().map_err(|_| "bad exactframerate")?, d.parse().map_err(|_| "bad exactframerate")?),
                None => (rate.parse().map_err(|_| "bad exactframerate")?, 1),
            };
            Media::Video((num("width")?, num("height")?, rate, fmtp(first, "interlace").is_some()))
        }
        ("audio", "l24" | "l16") => {
            let channels = rtpmap.split('/').nth(2).and_then(|c| c.parse().ok()).unwrap_or(1);
            let bits = if encoding == "l24" { 24 } else { 16 };
            Media::Audio { channels, bits, ptime_us: (first.ptime.unwrap_or(1.0) * 1000.0).round() as u32 }
        }
        ("video", "smpte291") => {
            let rate = fmtp(first, "exactframerate").and_then(|r| match r.split_once('/') {
                Some((n, d)) => Some((n.parse().ok()?, d.parse().ok()?)),
                None => Some((r.parse().ok()?, 1)),
            });
            Media::Data { rate, interlaced: fmtp(first, "interlace").is_some() }
        }
        (kind, enc) => Media::Other(format!("{kind}/{enc}")),
    };
    let legs = ms
        .iter()
        .map(|m| {
            let group = m.c.or(session_c).ok_or("the SDP has no address (c=)")?;
            Ok(SdpLeg { group, port: m.port, source: m.source.or(origin) })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Sdp { legs, media, payload_type: first.pt })
}

/// Le nom d'une vidéo pour les réglages (« 1080i50 », « 720p59.94 ») : en entrelacé, les trames
/// par seconde.
pub fn format_name((_, h, (n, d), interlaced): VideoFormat) -> String {
    let r = n as f64 / d.max(1) as f64 * if interlaced { 2.0 } else { 1.0 };
    let r = (r * 100.0).round() / 100.0;
    format!("{h}{}{}", if interlaced { 'i' } else { 'p' }, if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_round_trip() {
        let text = make(&SenderSdp {
            name: "Programme",
            session: 1,
            version: 2,
            legs: vec![("239.1.1.1".parse().unwrap(), 5004, "10.0.0.1".parse().unwrap()), ("239.2.1.1".parse().unwrap(), 5004, "10.0.1.1".parse().unwrap())],
            media: Media::Video((1920, 1080, (25, 1), true)),
            payload_type: 96,
            refclk: "ptp=IEEE1588-2008:AC-1F-6B-FF-FE-0A-0B-0C:127".into(),
        });
        assert!(text.contains("a=group:DUP primary secondary"));
        let p = parse(&text).unwrap();
        assert_eq!(p.legs.len(), 2);
        assert_eq!(p.legs[1], SdpLeg { group: "239.2.1.1".parse().unwrap(), port: 5004, source: Some("10.0.1.1".parse().unwrap()) });
        assert_eq!(p.media, Media::Video((1920, 1080, (25, 1), true)));
        assert_eq!(format_name((1920, 1080, (25, 1), true)), "1080i50");
        assert_eq!(format_name((1280, 720, (60000, 1001), false)), "720p59.94");
    }

    #[test]
    fn audio_from_another_maker() {
        let text = "v=0\no=- 123 1 IN IP4 192.168.10.5\ns=Mic 1\nt=0 0\nm=audio 5004 RTP/AVP 98\nc=IN IP4 239.69.0.12/32\n\
                    a=rtpmap:98 L24/48000/8\na=ptime:0.125\na=ts-refclk:ptp=IEEE1588-2008:00-1D-C1-FF-FE-12-34-56:0\na=mediaclk:direct=0\n";
        let p = parse(text).unwrap();
        assert_eq!(p.media, Media::Audio { channels: 8, bits: 24, ptime_us: 125 });
        assert_eq!(p.legs, vec![SdpLeg { group: "239.69.0.12".parse().unwrap(), port: 5004, source: Some("192.168.10.5".parse().unwrap()) }]);
        assert_eq!(p.payload_type, 98);
    }

    #[test]
    fn ancillary_data() {
        let s = SenderSdp {
            name: "ANC",
            session: 1,
            version: 1,
            legs: vec![(Ipv4Addr::new(239, 10, 40, 1), 5004, Ipv4Addr::new(10, 0, 0, 2))],
            media: Media::Data { rate: Some((25, 1)), interlaced: true },
            payload_type: 100,
            refclk: "ptp=IEEE1588-2008:traceable".into(),
        };
        let text = make(&s);
        assert!(text.contains("a=rtpmap:100 smpte291/90000") && text.contains("exactframerate=25; interlace"), "{text}");
        let p = parse(&text).unwrap();
        assert_eq!(p.media, Media::Data { rate: Some((25, 1)), interlaced: true });
        assert_eq!((p.legs[0].group, p.legs[0].port, p.payload_type), (Ipv4Addr::new(239, 10, 40, 1), 5004, 100));
    }
}
