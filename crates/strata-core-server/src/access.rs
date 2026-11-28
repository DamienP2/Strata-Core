//! Qui a accès à quelle appli : chaque appli le dit elle-même (son point `access` dans sa fiche),
//! le cockpit ne montre que les applis permises. Les droits restent dans l'appli : le Core ne fait
//! que poser la question.
//!
//! Rien ne bloque : une appli sans point d'accès, ou qui ne répond pas (arrêtée, lente, réponse
//! illisible), est montrée — elle refusera elle-même ce qui n'est pas permis.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::{apps::Installed, auth::User, sso::enc, state::AppState};

/// Une réponse gardée 30 s (le cockpit se recharge souvent).
const KEEP: Duration = Duration::from_secs(30);
const WAIT: Duration = Duration::from_millis(1500);

/// La personne, telle que les applis la voient (les en-têtes `X-Strata-*`).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Who {
    pub id: String,
    pub username: String,
    pub name: String,
    pub admin: bool,
    pub groups: Vec<String>,
    pub group_names: Vec<String>,
    pub lang: String,
}

impl Who {
    pub fn of(s: &AppState, u: &User) -> Who {
        Who {
            id: u.id.clone(),
            username: u.username.clone(),
            name: u.name().to_string(),
            admin: u.admin,
            groups: u.groups.clone(),
            group_names: s.groups.names_of(&u.groups),
            lang: u.lang.clone(),
        }
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("x-strata-user-id", self.id.clone()),
            ("x-strata-user", enc(&self.username)),
            ("x-strata-user-name", enc(&self.name)),
            ("x-strata-admin", if self.admin { "1" } else { "0" }.into()),
            ("x-strata-groups", self.groups.join(",")),
            ("x-strata-group-names", self.group_names.iter().map(|n| enc(n)).collect::<Vec<_>>().join(",")),
            ("x-strata-lang", self.lang.clone()),
        ]
    }

    fn key(&self) -> String {
        format!("{}|{}|{}", self.id, self.admin, self.groups.join(","))
    }
}

/// (appli, personne) → (réponse, quand).
type Answers = HashMap<(String, String), (bool, Instant)>;
static CACHE: Mutex<Option<Answers>> = Mutex::new(None);

/// Ce que dit une réponse de l'appli : Some(oui/non), ou None (pas de réponse claire).
fn verdict(status: u16, body: &[u8]) -> Option<bool> {
    match status {
        401 | 403 => Some(false),
        200..=299 => match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(v) => v.get("access").and_then(|a| a.as_bool()).or(Some(true)),
            Err(_) => Some(true),
        },
        _ => None,
    }
}

fn ask(port: u16, path: &str, who: &Who) -> Option<bool> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(WAIT)).http_status_as_error(false).build().into();
    let mut req = agent.get(&format!("http://127.0.0.1:{port}{path}"));
    for (k, v) in who.headers() {
        req = req.header(k, &v);
    }
    let res = req.call().ok()?;
    let status = res.status().as_u16();
    let body = res.into_body().with_config().limit(64 << 10).read_to_vec().ok()?;
    verdict(status, &body)
}

/// Cette personne a-t-elle accès à cette appli ? (oui si l'appli ne sait pas le dire)
pub async fn allowed(app: &Installed, who: &Who) -> bool {
    let Some(path) = app.manifest.access.clone() else { return true };
    let key = (app.manifest.id.clone(), who.key());
    if let Some((v, at)) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(&key).copied()) {
        if at.elapsed() < KEEP {
            return v;
        }
    }
    let (port, who2) = (app.manifest.port, who.clone());
    let answer = tokio::task::spawn_blocking(move || ask(port, &path, &who2)).await.ok().flatten();
    match answer {
        Some(v) => {
            let mut c = CACHE.lock().unwrap();
            let c = c.get_or_insert_with(HashMap::new);
            c.retain(|_, (_, at)| at.elapsed() < KEEP);
            c.insert(key, (v, Instant::now()));
            v
        }
        None => {
            tracing::debug!("{} does not say who has access: shown", app.manifest.id);
            true
        }
    }
}

/// Parmi ces applis, celles permises à cette personne (questions posées en même temps).
pub async fn filter(apps: Vec<Installed>, who: &Who) -> Vec<Installed> {
    let checks = futures_util::future::join_all(apps.iter().map(|a| allowed(a, who))).await;
    apps.into_iter().zip(checks).filter(|(_, ok)| *ok).map(|(a, _)| a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_an_answer_says() {
        assert_eq!(verdict(200, br#"{"access":false}"#), Some(false));
        assert_eq!(verdict(200, br#"{"access":true}"#), Some(true));
        assert_eq!(verdict(204, b""), Some(true));
        assert_eq!(verdict(403, b""), Some(false));
        assert_eq!(verdict(500, b""), None, "a broken app is shown");
        assert_eq!(verdict(404, b""), None);
    }
}
