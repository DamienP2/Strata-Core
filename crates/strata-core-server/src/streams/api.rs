//! L'API des flux réseau :
//!
//! - Réglages (administrateurs) : `GET/PUT /api/streams`, aperçus des entrées et des sorties.
//! - Applis (depuis la machine) : déclarer, changer, retirer leurs sorties
//!   (`/api/streams/app/outputs/:id`) ; l'appli écrit ensuite son flux MXL.
//! - HLS : `GET /hls/<sortie>/index.m3u8`, servi tel que la sortie l'écrit (sans session : un
//!   lecteur vidéo le lit).
//! - HTML5 : `GET /live/<sortie>` (la page du lecteur) et `GET /live/<sortie>/stream.mp4` (le flux
//!   en MP4 fragmenté), sans session non plus.
use axum::{
    extract::{ConnectInfo, Path as UrlPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;

use super::{audio_of, input_flow, output_flow, Destination, OutputCfg, StreamsConfig};
use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::{CurrentUser, MaybeUser},
    guard,
    hardware::{mxl_of, preview},
    state::Shared,
};

/// Une clé de diffusion n'est jamais renvoyée : ce masque la remplace (et la garde à l'écriture).
pub const MASK: &str = "••••••••";

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/streams", get(read).put(write))
        .route("/api/streams/inputs/{id}/preview.jpg", get(input_preview))
        .route("/api/streams/outputs/{id}/preview.jpg", get(output_preview))
        .route("/api/streams/app/outputs", get(app_list))
        .route("/api/streams/app/outputs/{id}", post(app_put).delete(app_delete))
        .route("/hls/{id}/{file}", get(hls))
        .route("/live/{id}", get(live_page))
        .route("/live/{id}/", get(live_page))
        .route("/live/{id}/stream.mp4", get(live_stream))
        .route("/live/{id}/player.js", get(live_player))
}

fn mxl_id(name: &str) -> String {
    strata_mxl::flow_id(name).to_string()
}

/// Les réglages et l'état, clés masquées.
fn view(s: &Shared, cfg: &StreamsConfig) -> Value {
    let inputs: Vec<Value> = cfg
        .inputs
        .iter()
        .map(|i| {
            let mut shown = i.clone();
            shown.opts.mask(MASK);
            let mut v = serde_json::to_value(&shown).unwrap_or_default();
            v["mxlFlow"] = json!(mxl_id(&input_flow(&i.id)));
            v["audioFlow"] = json!(mxl_id(&audio_of(&input_flow(&i.id))));
            v["relayUrl"] = if i.relay && i.relay_port != 0 { json!(format!("udp://127.0.0.1:{}", i.relay_port)) } else { Value::Null };
            v["status"] = json!(s.streams.input_status(&i.id));
            v
        })
        .collect();
    let outputs: Vec<Value> = cfg
        .outputs
        .iter()
        .map(|o| {
            let mut o = o.clone();
            for d in &mut o.destinations {
                if !d.key.is_empty() {
                    d.key = MASK.into();
                }
                d.opts.mask(MASK);
            }
            let mut v = serde_json::to_value(&o).unwrap_or_default();
            let src = if o.source.trim().is_empty() { output_flow(&o.id) } else { o.source.clone() };
            v["mxlFlow"] = json!(mxl_id(&src));
            v["audioFlow"] = json!(mxl_id(&audio_of(&src)));
            v["hlsUrl"] = if o.destinations.iter().any(|d| d.url == "hls") { json!(format!("/hls/{}/index.m3u8", o.id)) } else { Value::Null };
            v["liveUrl"] = if o.destinations.iter().any(|d| d.url == "html5") { json!(format!("/live/{}/", o.id)) } else { Value::Null };
            v["status"] = json!(s.streams.output_status(&o.id));
            v
        })
        .collect();
    json!({
        "ffmpeg": s.streams.ffmpeg().ok(),
        "problem": s.streams.problem(),
        "inputs": inputs,
        "outputs": outputs,
        "sources": sources(s, cfg),
    })
}

/// Les flux du serveur qu'une sortie peut envoyer : entrées réseau, SDI, ST 2110 reçus.
fn sources(s: &Shared, cfg: &StreamsConfig) -> Vec<Value> {
    let mut v: Vec<Value> = cfg.inputs.iter().map(|i| json!({"flow": input_flow(&i.id), "label": i.name, "kind": "stream"})).collect();
    let hw = crate::hardware::HardwareConfig::load(&s.data_dir);
    for c in &hw.cards {
        for (n, k) in c.connectors.iter().enumerate() {
            if k.function == crate::hardware::Function::Input {
                let label = if k.name.is_empty() { format!("{} · SDI {}", c.name, n + 1) } else { format!("{} · SDI {} · {}", c.name, n + 1, k.name) };
                v.push(json!({"flow": format!("sdi/{}/{}", c.id, n + 1), "label": label, "kind": "sdi"}));
            }
        }
    }
    for f in hw.flows.iter().filter(|f| f.essence == crate::hardware::Essence::Video && f.direction == crate::hardware::Direction::Input) {
        v.push(json!({"flow": format!("2110/{}", f.id), "label": f.name, "kind": "2110"}));
    }
    v
}

/// Pour les applis (`/api/hardware/shared`) : les entrées (flux MXL à lire, relais local du flux
/// d'origine) et les sorties (flux MXL à écrire), sans les adresses ni les clés.
pub fn shared(s: &crate::state::AppState) -> Value {
    let cfg = StreamsConfig::load(&s.data_dir);
    let inputs: Vec<Value> = cfg
        .inputs
        .iter()
        .filter(|i| i.enabled)
        .map(|i| {
            let st = s.streams.input_status(&i.id);
            json!({
                "id": i.id,
                "name": i.name,
                "mxlFlow": mxl_id(&input_flow(&i.id)),
                "audioFlow": mxl_id(&audio_of(&input_flow(&i.id))),
                "relay": if i.relay && i.relay_port != 0 { json!(format!("udp://127.0.0.1:{}", i.relay_port)) } else { Value::Null },
                "format": st.as_ref().and_then(|x| x.format.clone()),
                "live": st.as_ref().is_some_and(|x| x.state == super::engine::State::Running),
            })
        })
        .collect();
    let outputs: Vec<Value> = cfg
        .outputs
        .iter()
        .filter(|o| o.enabled && o.source.trim().is_empty())
        .map(|o| {
            let st = s.streams.output_status(&o.id);
            json!({
                "id": o.id,
                "name": o.name,
                "app": o.app,
                "mxlFlow": mxl_id(&output_flow(&o.id)),
                "audioFlow": mxl_id(&audio_of(&output_flow(&o.id))),
                "format": st.as_ref().and_then(|x| x.format.clone()),
                "sending": st.as_ref().is_some_and(|x| x.state == super::engine::State::Running),
            })
        })
        .collect();
    json!({ "inputs": inputs, "outputs": outputs })
}

async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    blocking(move || {
        let cfg = StreamsConfig::load(&s2.data_dir);
        Json(view(&s2, &cfg))
    })
    .await
}

/// Les clés masquées reprennent leur valeur gardée.
fn keep_keys(new: &mut StreamsConfig, old: &StreamsConfig) {
    for o in &mut new.outputs {
        let before = old.outputs.iter().find(|x| x.id == o.id);
        for d in &mut o.destinations {
            let old_d = before.and_then(|b| b.destinations.iter().find(|x| x.id == d.id));
            if d.key == MASK {
                d.key = old_d.map(|x| x.key.clone()).unwrap_or_default();
            }
            d.opts.keep(old_d.map(|x| &x.opts), MASK);
        }
    }
    for i in &mut new.inputs {
        let old_i = old.inputs.iter().find(|x| x.id == i.id);
        i.opts.keep(old_i.map(|x| &x.opts), MASK);
    }
}

/// Les sorties déclarées par les applis vivent avec elles : celles déclarées depuis l'ouverture de
/// la page sont gardées, celles retirées entre-temps ne reviennent pas.
fn keep_app_outputs(new: &mut StreamsConfig, old: &StreamsConfig) {
    new.outputs.retain(|o| o.app.is_none() || old.outputs.iter().any(|x| x.id == o.id));
    for o in old.outputs.iter().filter(|o| o.app.is_some()) {
        if !new.outputs.iter().any(|x| x.id == o.id) {
            new.outputs.push(o.clone());
        }
    }
}

async fn write(State(s): State<Shared>, CurrentUser(me): CurrentUser, Json(mut cfg): Json<StreamsConfig>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    blocking(move || {
        let _w = super::write_lock();
        let old = StreamsConfig::load(&s2.data_dir);
        keep_app_outputs(&mut cfg, &old);
        keep_keys(&mut cfg, &old);
        cfg.check().map_err(bad)?;
        cfg.save(&s2.data_dir).map_err(bad)?;
        tracing::info!("network streams set by {}", me.username);
        super::reconcile(&s2);
        Ok(Json(view(&s2, &cfg)))
    })
    .await?
}

fn jpeg(j: std::sync::Arc<Vec<u8>>) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "no-store")], (*j).clone())
}

/// L'aperçu d'un flux MXL du serveur.
async fn mxl_preview(s: Shared, key: String, flow: String) -> ApiResult<impl IntoResponse> {
    let j = blocking(move || {
        let mxl = mxl_of(&s).ok_or("no MXL")?;
        s.previews.jpeg(&key, || preview::Feed::Mxl { mxl, id: strata_mxl::flow_id(&flow), reader: None })
    })
    .await?
    .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(jpeg(j))
}

async fn input_preview(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    mxl_preview(s, format!("stream-in/{id}"), input_flow(&id)).await
}

async fn output_preview(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<impl IntoResponse> {
    require(me.admin)?;
    let cfg = StreamsConfig::load(&s.data_dir);
    let o = cfg.outputs.iter().find(|o| o.id == id).ok_or((StatusCode::NOT_FOUND, "no such output".to_string()))?;
    let src = if o.source.trim().is_empty() { output_flow(&o.id) } else { o.source.clone() };
    mxl_preview(s, format!("stream-out/{id}"), src).await
}

// ---------- Les applis ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppOutput {
    app: String,
    #[serde(default)]
    name: String,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    format: String,
    #[serde(default)]
    video_kbps: Option<u32>,
    #[serde(default)]
    audio_kbps: Option<u32>,
    #[serde(default)]
    encoder: Option<String>,
    #[serde(default)]
    destinations: Vec<Destination>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct AppQuery {
    app: String,
}

/// Une appli de ce serveur, active (depuis la machine, ou un administrateur).
fn app_allowed(s: &Shared, peer: SocketAddr, headers: &HeaderMap, admin: bool, app: &str) -> ApiResult<()> {
    if !(guard::this_machine(peer, headers) || admin) {
        return Err((StatusCode::FORBIDDEN, "only the apps of this server".into()));
    }
    if !s.apps.get(app).is_some_and(|a| a.active) {
        return Err(bad(format!("no active app: {app}")));
    }
    Ok(())
}

/// L'identifiant d'une sortie d'appli : `<appli>-<id>`.
fn app_output_id(app: &str, id: &str) -> String {
    format!("{app}-{id}").to_ascii_lowercase()
}

fn app_view(s: &Shared, o: &OutputCfg) -> Value {
    json!({
        "id": o.id,
        "name": o.name,
        "enabled": o.enabled,
        "mxlFlow": mxl_id(&output_flow(&o.id)),
        "audioFlow": mxl_id(&audio_of(&output_flow(&o.id))),
        "destinations": o.destinations.iter().map(|d| json!({"id": d.id, "name": d.name, "enabled": d.enabled})).collect::<Vec<_>>(),
        "status": s.streams.output_status(&o.id),
    })
}

async fn app_list(
    State(s): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    MaybeUser(user): MaybeUser,
    Query(q): Query<AppQuery>,
) -> ApiResult<Json<Value>> {
    app_allowed(&s, peer, &headers, user.is_some_and(|u| u.admin), &q.app)?;
    let cfg = StreamsConfig::load(&s.data_dir);
    let list: Vec<Value> = cfg.outputs.iter().filter(|o| o.app.as_deref() == Some(q.app.as_str())).map(|o| app_view(&s, o)).collect();
    Ok(Json(json!({ "outputs": list })))
}

/// Déclare (ou change) une sortie de l'appli. Une clé absente garde la précédente.
async fn app_put(
    State(s): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    MaybeUser(user): MaybeUser,
    UrlPath(id): UrlPath<String>,
    Json(b): Json<AppOutput>,
) -> ApiResult<Json<Value>> {
    app_allowed(&s, peer, &headers, user.is_some_and(|u| u.admin), &b.app)?;
    let s2 = s.clone();
    blocking(move || {
        let _w = super::write_lock();
        let mut cfg = StreamsConfig::load(&s2.data_dir);
        let old = cfg.clone();
        let oid = app_output_id(&b.app, &id);
        if let Some(o) = cfg.outputs.iter().find(|o| o.id == oid) {
            if o.app.as_deref() != Some(b.app.as_str()) {
                return Err(bad(format!("the output {oid} belongs to someone else")));
            }
        }
        let mut o = OutputCfg {
            id: oid.clone(),
            name: if b.name.trim().is_empty() { id.clone() } else { b.name.clone() },
            enabled: b.enabled,
            format: b.format.clone(),
            destinations: b.destinations.clone(),
            app: Some(b.app.clone()),
            ..OutputCfg::default()
        };
        if let Some(k) = b.video_kbps {
            o.video_kbps = k;
        }
        if let Some(k) = b.audio_kbps {
            o.audio_kbps = k;
        }
        if let Some(e) = &b.encoder {
            o.encoder = e.clone();
        }
        // Une clé non redonnée : la précédente.
        for d in &mut o.destinations {
            if d.key.is_empty() {
                d.key = MASK.into();
            }
        }
        match cfg.outputs.iter_mut().find(|x| x.id == oid) {
            Some(x) => *x = o,
            None => cfg.outputs.push(o),
        }
        keep_keys(&mut cfg, &old);
        cfg.check().map_err(bad)?;
        cfg.save(&s2.data_dir).map_err(bad)?;
        super::reconcile(&s2);
        let o = cfg.outputs.iter().find(|x| x.id == oid).cloned().unwrap_or_default();
        Ok(Json(app_view(&s2, &o)))
    })
    .await?
}

async fn app_delete(
    State(s): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    MaybeUser(user): MaybeUser,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<AppQuery>,
) -> ApiResult<Json<Value>> {
    app_allowed(&s, peer, &headers, user.is_some_and(|u| u.admin), &q.app)?;
    let s2 = s.clone();
    blocking(move || {
        let _w = super::write_lock();
        let mut cfg = StreamsConfig::load(&s2.data_dir);
        let oid = app_output_id(&q.app, &id);
        let before = cfg.outputs.len();
        cfg.outputs.retain(|o| !(o.id == oid && o.app.as_deref() == Some(q.app.as_str())));
        if cfg.outputs.len() == before {
            return Err((StatusCode::NOT_FOUND, format!("no output {oid}")));
        }
        cfg.save(&s2.data_dir).map_err(bad)?;
        super::reconcile(&s2);
        Ok(Json(json!({ "ok": true })))
    })
    .await?
}

// ---------- HLS ----------

/// Un fichier HLS d'une sortie : la liste ou un segment, rien d'autre.
async fn hls(State(s): State<Shared>, UrlPath((id, file)): UrlPath<(String, String)>) -> ApiResult<impl IntoResponse> {
    let ok_id = !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let segment = file.strip_prefix("seg").and_then(|f| f.strip_suffix(".ts")).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if !ok_id || !(file == "index.m3u8" || segment) {
        return Err((StatusCode::NOT_FOUND, "not found".into()));
    }
    let cfg = StreamsConfig::load(&s.data_dir);
    if !cfg.outputs.iter().any(|o| o.id == id && o.enabled && o.destinations.iter().any(|d| d.enabled && d.url == "hls")) {
        return Err((StatusCode::NOT_FOUND, "not found".into()));
    }
    let path = s.data_dir.join("streams").join("hls").join(&id).join(&file);
    let body = tokio::fs::read(&path).await.map_err(|_| (StatusCode::NOT_FOUND, "not found".to_string()))?;
    let kind = if segment { "video/mp2t" } else { "application/vnd.apple.mpegurl" };
    Ok((
        [(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, if segment { "max-age=60" } else { "no-cache" }), (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        body,
    ))
}

// ---------- HTML5 ----------

/// La sortie a-t-elle une destination HTML5 en marche ? (son nom, pour la page)
fn live_output(s: &Shared, id: &str) -> Option<String> {
    let cfg = StreamsConfig::load(&s.data_dir);
    cfg.outputs.into_iter().find(|o| o.id == id && o.enabled && o.destinations.iter().any(|d| d.enabled && d.url == "html5")).map(|o| o.name)
}

async fn live_page(State(s): State<Shared>, UrlPath(id): UrlPath<String>, uri: axum::http::Uri) -> ApiResult<axum::response::Response> {
    let Some(name) = live_output(&s, &id) else { return Err((StatusCode::NOT_FOUND, "not found".into())) };
    // `stream.mp4` est relatif : la page doit finir par « / ».
    if !uri.path().ends_with('/') {
        return Ok(axum::response::Redirect::to(&format!("/live/{id}/")).into_response());
    }
    let codecs = s.streams.live.get(&id).and_then(|c| c.codecs());
    let page = super::live::player_page(&name, codecs.as_deref());
    Ok((
        [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache"), (header::CONTENT_SECURITY_POLICY, super::live::PLAYER_CSP)],
        page,
    )
        .into_response())
}

async fn live_player() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], super::live::PLAYER_JS)
}

/// Le flux d'un spectateur : l'en-tête, puis les fragments à partir d'une image clé.
async fn live_stream(State(s): State<Shared>, UrlPath(id): UrlPath<String>) -> ApiResult<impl IntoResponse> {
    use tokio::sync::broadcast::error::RecvError;
    if live_output(&s, &id).is_none() {
        return Err((StatusCode::NOT_FOUND, "not found".into()));
    }
    let Some((init, _, gen, rx)) = s.streams.live.get(&id).and_then(|c| c.join()) else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "the stream is not there yet".into()));
    };
    struct St {
        rx: tokio::sync::broadcast::Receiver<super::live::Frag>,
        init: Option<axum::body::Bytes>,
        gen: u64,
        synced: bool,
    }
    let start = St { rx, init: Some(init), gen, synced: false };
    let body = futures_util::stream::unfold(start, |mut st| async move {
        if let Some(i) = st.init.take() {
            return Some((Ok::<_, std::io::Error>(i), st));
        }
        loop {
            // Rien pendant 5 s : le flux est arrêté ; le lecteur se reconnecte.
            match tokio::time::timeout(std::time::Duration::from_secs(5), st.rx.recv()).await {
                Ok(Ok(f)) if f.gen != st.gen => return None,
                Ok(Ok(f)) => {
                    if !st.synced && !f.key {
                        continue;
                    }
                    st.synced = true;
                    return Some((Ok(f.data), st));
                }
                // En retard : on saute jusqu'à la prochaine image clé.
                Ok(Err(RecvError::Lagged(_))) => st.synced = false,
                Ok(Err(RecvError::Closed)) | Err(_) => return None,
            }
        }
    });
    Ok((
        [(header::CONTENT_TYPE, "video/mp4"), (header::CACHE_CONTROL, "no-store"), (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        axum::body::Body::from_stream(body),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_keys_keep_their_value() {
        let d = |key: &str| Destination { id: "d1".into(), url: "rtmp://x/live".into(), key: key.into(), enabled: true, ..Default::default() };
        let old = StreamsConfig { outputs: vec![OutputCfg { id: "p".into(), destinations: vec![d("secret")], ..Default::default() }], ..Default::default() };
        let mut new = StreamsConfig { outputs: vec![OutputCfg { id: "p".into(), destinations: vec![d(MASK)], ..Default::default() }], ..Default::default() };
        keep_keys(&mut new, &old);
        assert_eq!(new.outputs[0].destinations[0].key, "secret");
        let mut changed =
            StreamsConfig { outputs: vec![OutputCfg { id: "p".into(), destinations: vec![d("new")], ..Default::default() }], ..Default::default() };
        keep_keys(&mut changed, &old);
        assert_eq!(changed.outputs[0].destinations[0].key, "new");
    }
}
