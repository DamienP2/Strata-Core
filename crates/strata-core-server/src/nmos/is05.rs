//! IS-05 v1.1 : l'API Connection. Un contrôleur prépare (`staged`) puis active un branchement,
//! tout de suite ou à une heure (TAI) donnée ; `active` dit ce qui tourne.
//!
//! - **Récepteurs** : le contrôleur donne les adresses ou le SDP de l'émetteur ; à l'activation,
//!   le flux reçu prend ces adresses (et le format du SDP) dans les réglages, et seul ce flux est
//!   relancé.
//! - **Émetteurs** : leurs adresses sont celles réglées à la main (les contraintes n'en offrent pas
//!   d'autre) ; le contrôleur peut les couper ou les rallumer (`master_enable`, `rtp_enabled`).
use axum::{body::Bytes, http::Method, http::StatusCode};
use serde_json::{json, Value};
use std::{net::Ipv4Addr, time::Duration};
use uuid::Uuid;

use super::{ends, error, parse_ts, sdp, tai_now, ts, End, Reply, Saved};
use crate::{
    hardware::{decklink, Essence, HardwareConfig},
    state::{AppState, Shared},
};

const SENDER_KEYS: [&str; 5] = ["source_ip", "destination_ip", "source_port", "destination_port", "rtp_enabled"];
const RECEIVER_KEYS: [&str; 5] = ["source_ip", "multicast_ip", "interface_ip", "destination_port", "rtp_enabled"];
const DEFAULT_PORT: u16 = 5004;

fn nulls() -> Value {
    json!({ "mode": null, "requested_time": null, "activation_time": null })
}

fn find(s: &AppState, sender: bool, id: &str) -> Option<(End, HardwareConfig, Saved)> {
    let cfg = HardwareConfig::load(&s.data_dir);
    let saved = Saved::load(&s.data_dir);
    let e = ends(&cfg, saved.node()).into_iter().find(|e| e.sender == sender && e.id.to_string() == id)?;
    Some((e, cfg, saved))
}

/// L'adresse de chaque patte : le rouge (port 1), puis le bleu.
fn leg_addr(e: &End, i: usize) -> Option<std::net::SocketAddrV4> {
    if i == 0 {
        Some(e.flow.red)
    } else {
        e.flow.blue
    }
}

fn active(s: &AppState, e: &End, saved: &Saved) -> Value {
    let f = &e.flow;
    let link = saved.links.get(&f.id).cloned().unwrap_or_default();
    let legs: Vec<Value> = (0..e.ifaces.len())
        .map(|i| {
            let a = leg_addr(e, i);
            let port = a.map_or(f.red.port(), |a| a.port());
            let iface_ip = e.ifaces[i].1.unwrap_or(Ipv4Addr::UNSPECIFIED).to_string();
            if e.sender {
                json!({ "source_ip": iface_ip, "destination_ip": a.map(|a| a.ip().to_string()), "source_port": port, "destination_port": port, "rtp_enabled": f.enabled })
            } else {
                json!({
                    "source_ip": link.sources.get(i).copied().flatten().map(|ip| ip.to_string()),
                    "multicast_ip": a.map(|a| a.ip().to_string()),
                    "interface_ip": iface_ip,
                    "destination_port": port,
                    "rtp_enabled": f.enabled && a.is_some(),
                })
            }
        })
        .collect();
    let activation = s.nmos.lock().activations.get(&e.id).cloned().unwrap_or_else(nulls);
    if e.sender {
        json!({ "receiver_id": link.peer, "master_enable": f.enabled, "activation": activation, "transport_params": legs })
    } else {
        json!({
            "sender_id": link.peer,
            "master_enable": f.enabled,
            "activation": activation,
            "transport_file": { "data": link.sdp, "type": link.sdp.as_ref().map(|_| "application/sdp") },
            "transport_params": legs,
        })
    }
}

fn staged(s: &AppState, e: &End, saved: &Saved) -> Value {
    {
        let mut inner = s.nmos.lock();
        match inner.staged.get(&e.id) {
            // Préparé avant que les pattes changent (2022-7 réglé, port ajouté) : refait.
            Some(v) if v["transport_params"].as_array().map(Vec::len) != Some(e.ifaces.len()) => {
                inner.staged.remove(&e.id);
            }
            Some(v) => return v.clone(),
            None => {}
        }
    }
    let mut v = active(s, e, saved);
    v["activation"] = nulls();
    v
}

/// Ce que chaque patte accepte. Les émetteurs : leurs adresses réglées seulement.
fn constraints(e: &End) -> Value {
    let legs: Vec<Value> = (0..e.ifaces.len())
        .map(|i| {
            let ip = e.ifaces[i].1.map(|ip| json!({ "enum": [ip.to_string()] })).unwrap_or(json!({}));
            if e.sender {
                let a = leg_addr(e, i).unwrap_or(e.flow.red);
                json!({
                    "source_ip": ip,
                    "destination_ip": { "enum": [a.ip().to_string()] },
                    "source_port": { "enum": [a.port()] },
                    "destination_port": { "enum": [a.port()] },
                    "rtp_enabled": {},
                })
            } else {
                json!({ "source_ip": {}, "multicast_ip": {}, "interface_ip": ip, "destination_port": {}, "rtp_enabled": {} })
            }
        })
        .collect();
    json!(legs)
}

fn transport_file(s: &AppState, e: &End, cfg: &HardwareConfig) -> Reply {
    let f = &e.flow;
    let legs: Vec<(Ipv4Addr, u16, Ipv4Addr)> =
        (0..e.ifaces.len()).filter_map(|i| leg_addr(e, i).map(|a| (*a.ip(), a.port(), e.ifaces[i].1.unwrap_or(Ipv4Addr::UNSPECIFIED)))).collect();
    let media = match f.essence {
        Essence::Audio => sdp::Media::Audio { channels: f.channels, bits: f.bits, ptime_us: f.ptime_us },
        Essence::Data => {
            let (_, _, rate, interlaced) = cfg.video_of(f).unwrap_or((1920, 1080, (25, 1), true));
            sdp::Media::Data { rate: Some(rate), interlaced }
        }
        Essence::Video => sdp::Media::Video(cfg.video_of(f).unwrap_or((1920, 1080, (25, 1), true))),
    };
    let version = s.nmos.lock().versions.get(&e.id).and_then(|v| v.1.split(':').next().and_then(|x| x.parse().ok())).unwrap_or(1);
    let session = u64::from_be_bytes(e.id.as_bytes()[..8].try_into().unwrap_or_default()) >> 1;
    let iface = e.ifaces.first().map(|i| i.0.clone()).unwrap_or_default();
    Reply::Sdp(sdp::make(&sdp::SenderSdp { name: &f.name, session, version, legs, media, payload_type: f.pt(), refclk: super::refclk(s, cfg, &iface) }))
}

pub(crate) fn read(s: &AppState, rest: &[&str]) -> Reply {
    let names = |l: &[&str]| Reply::Json(StatusCode::OK, json!(l));
    match rest {
        [] => names(&["v1.1/"]),
        ["v1.1"] => names(&["bulk/", "single/"]),
        ["v1.1", "bulk"] => names(&["senders/", "receivers/"]),
        ["v1.1", "bulk", "senders" | "receivers"] => error(StatusCode::METHOD_NOT_ALLOWED, "POST only"),
        ["v1.1", "single"] => names(&["senders/", "receivers/"]),
        ["v1.1", "single", kind @ ("senders" | "receivers"), more @ ..] => {
            let sender = *kind == "senders";
            if more.is_empty() {
                let cfg = HardwareConfig::load(&s.data_dir);
                let list: Vec<String> =
                    ends(&cfg, Saved::load(&s.data_dir).node()).into_iter().filter(|e| e.sender == sender).map(|e| format!("{}/", e.id)).collect();
                return Reply::Json(StatusCode::OK, json!(list));
            }
            let Some((e, cfg, saved)) = find(s, sender, more[0]) else { return error(StatusCode::NOT_FOUND, "no such sender or receiver") };
            match &more[1..] {
                [] if sender => names(&["constraints/", "staged/", "active/", "transportfile/", "transporttype/"]),
                [] => names(&["constraints/", "staged/", "active/", "transporttype/"]),
                ["constraints"] => Reply::Json(StatusCode::OK, constraints(&e)),
                ["staged"] => Reply::Json(StatusCode::OK, staged(s, &e, &saved)),
                ["active"] => Reply::Json(StatusCode::OK, active(s, &e, &saved)),
                ["transporttype"] => Reply::Json(StatusCode::OK, json!("urn:x-nmos:transport:rtp")),
                ["transportfile"] if sender => transport_file(s, &e, &cfg),
                _ => error(StatusCode::NOT_FOUND, "not found"),
            }
        }
        _ => error(StatusCode::NOT_FOUND, "not found"),
    }
}

/// PATCH `staged`, POST `bulk`.
pub(crate) async fn write(s: &Shared, method: &Method, segs: &[String], body: &Bytes) -> Reply {
    let p: Vec<&str> = segs.iter().map(String::as_str).collect();
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return error(StatusCode::BAD_REQUEST, "the body is not JSON") };
    match (method, p.as_slice()) {
        (&Method::PATCH, ["x-nmos", "connection", "v1.1", "single", kind @ ("senders" | "receivers"), id, "staged"]) => {
            patch(s, *kind == "senders", id, v).await
        }
        (&Method::POST, ["x-nmos", "connection", "v1.1", "bulk", kind @ ("senders" | "receivers")]) => {
            let Some(items) = v.as_array() else { return error(StatusCode::BAD_REQUEST, "a list of {id, params}") };
            let mut out = Vec::new();
            for it in items {
                let id = it["id"].as_str().unwrap_or("").to_string();
                let r = patch(s, *kind == "senders", &id, it["params"].clone()).await;
                out.push(match r {
                    Reply::Json(code, _) if code.is_success() => json!({ "id": id, "code": code.as_u16() }),
                    Reply::Json(code, body) => json!({ "id": id, "code": code.as_u16(), "error": body["error"] }),
                    Reply::Sdp(_) => json!({ "id": id, "code": 500 }),
                });
            }
            Reply::Json(StatusCode::OK, json!(out))
        }
        (_, ["x-nmos", "connection", ..]) => error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
        _ => error(StatusCode::NOT_FOUND, "not found"),
    }
}

/// Que faire après la préparation.
enum Act {
    Nothing,
    Immediate,
    /// Le mode, l'heure demandée, l'heure TAI d'activation.
    Scheduled(String, String, Duration),
}

fn is_auto(v: &Value) -> bool {
    v.as_str() == Some("auto")
}

/// Vérifie une valeur d'une patte.
fn check_param(sender: bool, key: &str, v: &Value, c: &Value) -> Result<(), String> {
    let ip_ok = |v: &Value| v.as_str().is_some_and(|x| x.parse::<Ipv4Addr>().is_ok());
    let ok = match key {
        "rtp_enabled" => v.is_boolean(),
        "source_port" | "destination_port" => is_auto(v) || v.as_u64().is_some_and(|p| (1..=65535).contains(&p)),
        // Récepteur : l'émetteur et le groupe peuvent être vides (null) ; pas « auto ».
        "source_ip" | "multicast_ip" if !sender => v.is_null() || ip_ok(v),
        _ => is_auto(v) || ip_ok(v),
    };
    if !ok {
        return Err(format!("{key}: bad value {v}"));
    }
    if key == "multicast_ip" {
        if let Some(ip) = v.as_str().and_then(|x| x.parse::<Ipv4Addr>().ok()) {
            if !ip.is_multicast() {
                return Err(format!("multicast_ip: {ip} is not a multicast address"));
            }
        }
    }
    if let Some(list) = c["enum"].as_array() {
        if !is_auto(v) && !v.is_null() && !list.contains(v) {
            return Err(format!("{key}: {v} is not allowed (here: {})", Value::Array(list.clone())));
        }
    }
    Ok(())
}

/// Prépare : vérifie le PATCH et le range dans `staged`.
fn stage(s: &AppState, sender: bool, id: &str, body: &Value) -> Result<(Uuid, Value, Act, u32), Reply> {
    let bad = |why: String| error(StatusCode::BAD_REQUEST, why);
    let Some(obj) = body.as_object() else { return Err(bad("the body is an object".into())) };
    let Some((e, cfg, saved)) = find(s, sender, id) else { return Err(error(StatusCode::NOT_FOUND, "no such sender or receiver")) };
    let peer_key = if sender { "receiver_id" } else { "sender_id" };
    for k in obj.keys() {
        let known = [peer_key, "master_enable", "activation", "transport_params"].contains(&k.as_str()) || (!sender && k == "transport_file");
        if !known {
            return Err(bad(format!("unknown field {k}")));
        }
    }
    let cancel = obj.get("activation").is_some_and(|a| a.get("mode").is_some_and(Value::is_null));
    let mut st = staged(s, &e, &saved);
    if s.nmos.lock().pending.contains_key(&e.id) && !cancel {
        return Err(error(StatusCode::LOCKED, "an activation is scheduled: cancel it first"));
    }
    // `staged` a une patte par interface : les index ci-dessous sont dans les bornes.
    debug_assert_eq!(st["transport_params"].as_array().map(Vec::len), Some(e.ifaces.len()));
    if let Some(v) = obj.get(peer_key) {
        if !(v.is_null() || v.as_str().is_some_and(|x| x.parse::<Uuid>().is_ok())) {
            return Err(bad(format!("{peer_key} is a UUID or null")));
        }
        st[peer_key] = v.clone();
    }
    if let Some(v) = obj.get("master_enable") {
        if !v.is_boolean() {
            return Err(bad("master_enable is true or false".into()));
        }
        st["master_enable"] = v.clone();
    }
    let legs = e.ifaces.len();
    let cons = constraints(&e);
    // Le SDP de l'émetteur : il donne les adresses de chaque patte.
    if let Some(tf) = obj.get("transport_file") {
        match tf.get("data") {
            Some(Value::String(text)) => {
                if tf.get("type").and_then(Value::as_str) != Some("application/sdp") {
                    return Err(bad("transport_file.type must be application/sdp".into()));
                }
                let p = sdp::parse(text).map_err(bad)?;
                let fits = matches!(
                    (&p.media, e.flow.essence),
                    (sdp::Media::Video(_), Essence::Video) | (sdp::Media::Audio { .. }, Essence::Audio) | (sdp::Media::Data { .. }, Essence::Data)
                );
                if !fits {
                    return Err(bad(format!("{} cannot receive this flow", e.flow.name)));
                }
                for i in 0..legs {
                    let tp = &mut st["transport_params"][i];
                    match p.legs.get(i) {
                        Some(l) => {
                            tp["multicast_ip"] = json!(l.group.to_string());
                            tp["destination_port"] = json!(l.port);
                            tp["source_ip"] = json!(l.source.map(|x| x.to_string()));
                            tp["rtp_enabled"] = json!(true);
                        }
                        None => tp["rtp_enabled"] = json!(false),
                    }
                }
                st["transport_file"] = json!({ "data": text, "type": "application/sdp" });
            }
            Some(Value::Null) | None => st["transport_file"] = json!({ "data": null, "type": null }),
            Some(_) => return Err(bad("transport_file.data is the SDP text or null".into())),
        }
    }
    if let Some(tps) = obj.get("transport_params") {
        let Some(list) = tps.as_array() else { return Err(bad("transport_params is a list".into())) };
        if list.len() > legs {
            return Err(bad(format!("{} has {legs} leg(s)", e.flow.name)));
        }
        let keys = if sender { SENDER_KEYS } else { RECEIVER_KEYS };
        for (i, tp) in list.iter().enumerate() {
            let Some(tp) = tp.as_object() else { return Err(bad("each leg is an object".into())) };
            for (k, v) in tp {
                if !keys.contains(&k.as_str()) {
                    return Err(bad(format!("unknown transport parameter {k}")));
                }
                check_param(sender, k, v, &cons[i][k]).map_err(bad)?;
                st["transport_params"][i][k] = v.clone();
            }
        }
    }
    let tai = cfg.ptp.tai_offset;
    let act = match obj.get("activation") {
        None => Act::Nothing,
        Some(a) => {
            let mode = a.get("mode").cloned().unwrap_or(Value::Null);
            let req = a.get("requested_time").cloned().unwrap_or(Value::Null);
            match mode.as_str() {
                None if mode.is_null() => {
                    if let Some(Some(t)) = s.nmos.lock().pending.remove(&e.id) {
                        t.abort();
                    }
                    st["activation"] = nulls();
                    Act::Nothing
                }
                Some("activate_immediate") => Act::Immediate,
                Some(m @ ("activate_scheduled_absolute" | "activate_scheduled_relative")) => {
                    let Some(t) = req.as_str().and_then(parse_ts) else { return Err(bad("requested_time is \"seconds:nanoseconds\"".into())) };
                    let now = tai_now(tai);
                    let at = if m == "activate_scheduled_relative" { now.checked_add(t) } else { Some(t) };
                    // Au plus une semaine d'avance (au-delà : une erreur de saisie, et un minuteur qui déborde).
                    let Some(at) = at.filter(|at| at.saturating_sub(now) <= Duration::from_secs(7 * 86400)) else {
                        return Err(bad("requested_time is too far ahead (a week at most)".into()));
                    };
                    st["activation"] = json!({ "mode": m, "requested_time": req, "activation_time": ts(at) });
                    Act::Scheduled(m.to_string(), req.as_str().unwrap_or_default().to_string(), at)
                }
                _ => return Err(bad(format!("unknown activation mode {mode}"))),
            }
        }
    };
    // Vérifié et réservé sous le même verrou : deux activations programmées à la fois, l'une
    // reçoit 423.
    let mut inner = s.nmos.lock();
    if inner.pending.contains_key(&e.id) && !cancel {
        return Err(error(StatusCode::LOCKED, "an activation is scheduled: cancel it first"));
    }
    if matches!(act, Act::Scheduled(..)) {
        inner.pending.insert(e.id, None);
    }
    inner.staged.insert(e.id, st.clone());
    drop(inner);
    Ok((e.id, st, act, tai))
}

async fn patch(s: &Shared, sender: bool, id: &str, body: Value) -> Reply {
    let (s2, id2) = (s.clone(), id.to_string());
    let staged = match tokio::task::spawn_blocking(move || stage(&s2, sender, &id2, &body)).await {
        Ok(Ok(x)) => x,
        Ok(Err(r)) => return r,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let (uid, mut st, act, tai) = staged;
    match act {
        Act::Nothing => Reply::Json(StatusCode::OK, st),
        Act::Immediate => {
            let s2 = s.clone();
            let activation = json!({ "mode": "activate_immediate", "requested_time": null, "activation_time": ts(tai_now(tai)) });
            let a2 = activation.clone();
            match tokio::task::spawn_blocking(move || finish(&s2, uid, a2)).await {
                Ok(Ok(())) => {
                    st["activation"] = activation;
                    Reply::Json(StatusCode::OK, st)
                }
                Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
        }
        Act::Scheduled(mode, req, at) => {
            let delay = at.saturating_sub(tai_now(tai));
            let activation = json!({ "mode": mode, "requested_time": req, "activation_time": ts(at) });
            let s2 = s.clone();
            // Rangé sous le verrou : la tâche ne peut pas finir avant d'être notée.
            let mut inner = s.nmos.lock();
            let task = tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let s3 = s2.clone();
                let r = tokio::task::spawn_blocking(move || finish(&s3, uid, activation)).await;
                if let Ok(Err(e)) = r {
                    tracing::warn!("NMOS: scheduled activation of {uid} failed: {e}");
                }
                s2.nmos.lock().pending.remove(&uid);
            });
            // Annulée entre-temps : la tâche ne part pas.
            match inner.pending.get_mut(&uid) {
                Some(slot) => *slot = Some(task),
                None => task.abort(),
            }
            Reply::Json(StatusCode::ACCEPTED, st)
        }
    }
}

/// Active ce qui est préparé, et le note.
fn finish(s: &AppState, id: Uuid, activation: Value) -> Result<(), String> {
    let r = activate(s, id);
    let mut inner = s.nmos.lock();
    if let Some(st) = inner.staged.get_mut(&id) {
        st["activation"] = nulls();
    }
    if r.is_ok() {
        inner.activations.insert(id, activation);
    }
    drop(inner);
    s.nmos.current(s);
    super::announce(s);
    r
}

/// Écrit le branchement dans les réglages, puis relance le flux.
fn activate(s: &AppState, id: Uuid) -> Result<(), String> {
    let _w = crate::hardware::write_lock();
    let st = s.nmos.lock().staged.get(&id).cloned().ok_or("nothing staged")?;
    let mut cfg = HardwareConfig::load(&s.data_dir);
    let mut saved = Saved::load(&s.data_dir);
    let e = ends(&cfg, saved.node()).into_iter().find(|e| e.id == id).ok_or("this flow is gone")?;
    let master = st["master_enable"].as_bool().unwrap_or(false);
    let tps = st["transport_params"].as_array().cloned().unwrap_or_default();
    let link = saved.links.entry(e.flow.id.clone()).or_default();
    let flow = cfg.flows.iter_mut().find(|f| f.id == e.flow.id).ok_or("this flow is gone")?;
    if e.sender {
        flow.enabled = master && tps.iter().any(|t| t["rtp_enabled"].as_bool() == Some(true));
        link.peer = st["receiver_id"].as_str().and_then(|x| x.parse().ok());
    } else {
        let leg = |i: usize| -> Option<(std::net::SocketAddrV4, Option<Ipv4Addr>)> {
            let t = tps.get(i)?;
            if t["rtp_enabled"].as_bool() != Some(true) {
                return None;
            }
            let ip: Ipv4Addr = t["multicast_ip"].as_str()?.parse().ok()?;
            let port = t["destination_port"].as_u64().map_or(DEFAULT_PORT, |p| p as u16);
            Some((std::net::SocketAddrV4::new(ip, port), t["source_ip"].as_str().and_then(|x| x.parse().ok())))
        };
        let (l0, l1) = (leg(0), leg(1));
        if master && (l0.is_some() || l1.is_some()) {
            let Some((red, _)) = l0 else { return Err("the first leg (red network) must be enabled with a multicast address".into()) };
            flow.red = red;
            flow.blue = l1.map(|l| l.0);
            flow.enabled = true;
        } else {
            flow.enabled = false;
        }
        // Le format de l'émetteur, d'après son SDP.
        let sdp_text = st["transport_file"]["data"].as_str().map(str::to_string);
        if let Some(p) = sdp_text.as_deref().and_then(|t| sdp::parse(t).ok()) {
            match p.media {
                sdp::Media::Video(v) => flow.format = sdp::format_name(v),
                sdp::Media::Audio { channels, bits, ptime_us } => {
                    flow.channels = channels;
                    flow.bits = bits;
                    flow.ptime_us = ptime_us;
                }
                // La cadence de l'émetteur, la taille d'image gardée (elle ne compte pas ici).
                sdp::Media::Data { rate: Some(r), interlaced } => {
                    let (w, h, _, _) = crate::hardware::parse_format(&flow.format).unwrap_or((1920, 1080, r, interlaced));
                    flow.format = sdp::format_name((w, h, r, interlaced));
                }
                sdp::Media::Data { rate: None, .. } | sdp::Media::Other(_) => {}
            }
        }
        link.peer = st["sender_id"].as_str().and_then(|x| x.parse().ok());
        link.sources = vec![l0.and_then(|l| l.1), l1.and_then(|l| l.1)];
        link.sdp = sdp_text;
    }
    let (cards, _, _) = decklink::detect();
    cfg.check(&cards)?;
    strata_core::files::write_json(&HardwareConfig::path(&s.data_dir), &cfg, false).map_err(|e| format!("cannot save the hardware settings: {e}"))?;
    saved.save(&s.data_dir);
    tracing::info!("NMOS: {} {} activated", if e.sender { "sender" } else { "receiver" }, e.flow.name);
    crate::hardware::reconcile_inputs(s);
    Ok(())
}
