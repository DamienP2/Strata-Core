//! Les fonctions du Core qu'on active ou qu'on coupe (Réglages › Fonctions) : ST 2110, JPEG XS,
//! IPMX et ses options, Dante, NDI, flux réseau.
//!
//! Une fonction coupée disparaît des réglages et ne tourne plus (ses flux s'arrêtent, rien n'est
//! écouté ni annoncé). Une fonction peut en demander une autre (JPEG XS demande ST 2110) : elle
//! ne marche que si celle-ci est active aussi. Être active ne bloque jamais rien : une
//! bibliothèque ou une licence qui manque est dite (`available`), le reste du Core continue.
//!
//! Les choix sont dans `features.json` ; une fonction jamais réglée prend sa valeur par défaut.
//! Les licences (HDCP) sont dans `licenses/<fonction>` (0600).
use axum::{
    body::Bytes,
    extract::{Path as UrlPath, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::{
    api::{bad, blocking, require, ApiResult},
    auth::CurrentUser,
    state::{AppState, Shared},
};

/// Une fonction : son identifiant, son nom, celles qu'elle demande, sa valeur par défaut, et si
/// elle demande une licence.
pub struct Def {
    pub id: &'static str,
    pub name: &'static str,
    pub needs: &'static [&'static str],
    pub default: bool,
    pub license: bool,
}

pub const ST2110: &str = "st2110";
pub const JPEGXS: &str = "jpegxs";
pub const IPMX: &str = "ipmx";
pub const IPMX_PRIVACY: &str = "ipmx-privacy";
pub const HDCP: &str = "hdcp";
pub const DANTE: &str = "dante";
pub const NDI: &str = "ndi";
pub const STREAMS: &str = "streams";

pub const DEFS: &[Def] = &[
    Def { id: ST2110, name: "ST 2110", needs: &[], default: true, license: false },
    Def { id: JPEGXS, name: "JPEG XS (ST 2110-22)", needs: &[ST2110], default: false, license: false },
    Def { id: IPMX, name: "IPMX", needs: &[ST2110], default: false, license: false },
    Def { id: IPMX_PRIVACY, name: "IPMX privacy encryption (PEP)", needs: &[IPMX], default: false, license: false },
    Def { id: HDCP, name: "HDCP (IPMX HKEP)", needs: &[IPMX], default: false, license: true },
    Def { id: DANTE, name: "Dante / AES67", needs: &[], default: true, license: false },
    Def { id: NDI, name: "NDI", needs: &[], default: true, license: false },
    Def { id: STREAMS, name: "Network streams", needs: &[], default: true, license: false },
];

fn def(id: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.id == id)
}

/// Les choix (relus à chaque changement).
pub struct Features {
    chosen: RwLock<HashMap<String, bool>>,
}

impl Features {
    pub fn load(data_dir: &Path) -> Features {
        let chosen = std::fs::read_to_string(path(data_dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Features { chosen: RwLock::new(chosen) }
    }

    /// Réglée active (sans regarder celles qu'elle demande).
    fn chosen(&self, id: &str) -> bool {
        let c = self.chosen.read().unwrap_or_else(|e| e.into_inner());
        c.get(id).copied().unwrap_or_else(|| def(id).is_some_and(|d| d.default))
    }

    /// Active, et celles qu'elle demande aussi.
    pub fn on(&self, id: &str) -> bool {
        self.chosen(id) && def(id).is_some_and(|d| d.needs.iter().all(|n| self.on(n)))
    }

    fn set(&self, data_dir: &Path, id: &str, on: bool) -> Result<(), String> {
        let mut c = self.chosen.write().unwrap_or_else(|e| e.into_inner());
        c.insert(id.to_string(), on);
        strata_core::files::write_json(&path(data_dir), &*c, false).map_err(|e| e.to_string())
    }
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("features.json")
}

/// Où est gardée la licence d'une fonction.
pub fn license_path(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("licenses").join(id)
}

/// Le greffon JPEG XS de MTL, posé à côté de libmtl.
pub fn jpegxs_plugin(s: &AppState) -> Option<PathBuf> {
    let lib = s.mtl.get()?.as_ref().ok()?;
    let p = lib.parent()?.join("libst_plugin_st22_svt_jpeg_xs.so");
    p.is_file().then_some(p)
}

/// Peut-elle marcher sur ce serveur ? Sinon, pourquoi (en anglais : traduit par l'interface).
fn available(s: &AppState, id: &str) -> Result<(), String> {
    let mtl = || match s.mtl.get() {
        Some(Ok(_)) => Ok(()),
        Some(Err(e)) => Err(format!("MTL is not loaded: {e}")),
        None => Err("MTL is not loaded yet".to_string()),
    };
    match id {
        ST2110 | IPMX => mtl(),
        JPEGXS => {
            mtl()?;
            jpegxs_plugin(s).map(|_| ()).ok_or_else(|| "the JPEG XS plugin of MTL (SVT-JPEG-XS) is not installed".to_string())
        }
        IPMX_PRIVACY => Err("not in this version of the Core yet".to_string()),
        HDCP => {
            if !license_path(&s.data_dir, HDCP).is_file() {
                return Err("needs an HDCP license (DCP LLC keys): add it here".to_string());
            }
            Err("not in this version of the Core yet (the license is kept)".to_string())
        }
        NDI => crate::hardware::ndi::runtime().map(|_| ()),
        STREAMS => s.streams.ffmpeg().map(|_| ()),
        _ => Ok(()),
    }
}

/// L'état de toutes, pour les réglages et les applis.
pub fn view(s: &AppState) -> Value {
    let list: Vec<Value> = DEFS
        .iter()
        .map(|d| {
            let license = license_path(&s.data_dir, d.id);
            let meta = std::fs::metadata(&license).ok();
            let why = available(s, d.id).err();
            json!({
                "id": d.id,
                "name": d.name,
                "chosen": s.features.chosen(d.id),
                "on": s.features.on(d.id),
                "default": d.default,
                "needs": d.needs,
                "available": why.is_none(),
                "why": why,
                "license": if d.license {
                    json!({ "present": meta.is_some(), "bytes": meta.as_ref().map(|m| m.len()), "sha256": meta.and_then(|_| std::fs::read(&license).ok()).map(|b| fingerprint(&b)) })
                } else { Value::Null },
            })
        })
        .collect();
    json!({ "features": list })
}

/// Les 8 premiers octets du SHA-256, pour reconnaître une licence sans la montrer.
fn fingerprint(b: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, b).as_ref().iter().take(8).map(|x| format!("{x:02x}")).collect()
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/features", get(read))
        .route("/api/features/{id}", put(write))
        .route("/api/features/{id}/license", post(add_license).delete(remove_license))
}

async fn read(State(s): State<Shared>, CurrentUser(me): CurrentUser) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    let s2 = s.clone();
    blocking(move || Json(view(&s2))).await
}

#[derive(Deserialize)]
struct Set {
    on: bool,
}

/// Ce qui en dépend, remis en accord (bloquant).
fn apply(s: &Shared) {
    crate::hardware::reconcile_inputs(s);
    crate::hardware::sap::apply(s);
}

async fn write(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>, Json(b): Json<Set>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    if def(&id).is_none() {
        return Err(bad(format!("no such feature: {id}")));
    }
    let s2 = s.clone();
    let v = blocking(move || {
        s2.features.set(&s2.data_dir, &id, b.on).map_err(bad)?;
        tracing::info!("feature {id} {} by {}", if b.on { "on" } else { "off" }, me.username);
        Ok::<_, (axum::http::StatusCode, String)>(view(&s2))
    })
    .await??;
    // Les flux, l'écoute SAP, NMOS : à part, la réponse n'attend pas.
    let s3 = s.clone();
    tokio::spawn(async move {
        let s4 = s3.clone();
        let _ = tokio::task::spawn_blocking(move || apply(&s4)).await;
        crate::nmos::apply(&s3).await;
    });
    Ok(Json(v))
}

/// Une licence (le fichier tel que fourni : clés DCP pour HDCP), 1 Mo au plus.
async fn add_license(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>, body: Bytes) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    if !def(&id).is_some_and(|d| d.license) {
        return Err(bad(format!("{id} needs no license")));
    }
    if body.is_empty() || body.len() > 1 << 20 {
        return Err(bad("the license file is empty or too big (1 MB at most)"));
    }
    let s2 = s.clone();
    blocking(move || {
        let p = license_path(&s2.data_dir, &id);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| bad(e.to_string()))?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        strata_core::files::write_atomic(&p, &body, true).map_err(|e| bad(e.to_string()))?;
        tracing::info!("license for {id} added by {}", me.username);
        Ok(Json(view(&s2)))
    })
    .await?
}

async fn remove_license(State(s): State<Shared>, CurrentUser(me): CurrentUser, UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    require(me.admin)?;
    if !def(&id).is_some_and(|d| d.license) {
        return Err(bad(format!("{id} needs no license")));
    }
    let s2 = s.clone();
    blocking(move || {
        let _ = std::fs::remove_file(license_path(&s2.data_dir, &id));
        tracing::info!("license for {id} removed by {}", me.username);
        Json(view(&s2))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_feature_needs_the_ones_it_asks_for() {
        let dir = std::env::temp_dir().join(format!("strata-features-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = Features::load(&dir);
        assert!(f.on(ST2110) && !f.on(JPEGXS));
        f.set(&dir, JPEGXS, true).unwrap();
        assert!(f.on(JPEGXS));
        f.set(&dir, ST2110, false).unwrap();
        assert!(!f.on(JPEGXS) && f.chosen(JPEGXS));
        // Gardé.
        let g = Features::load(&dir);
        assert!(!g.on(ST2110) && g.chosen(JPEGXS));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
