//! L'interface web : embarquée dans le binaire (web/dist), ou servie depuis `--web-dir`. Une
//! adresse qui n'est pas un fichier renvoie index.html (les pages de l'interface). Repris de LiveCut.
use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebApp;

pub async fn embedded(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "no such route").into_response();
    }
    let (path, file) = match WebApp::get(path) {
        Some(f) if !path.is_empty() => (path, f),
        _ => match WebApp::get("index.html") {
            Some(f) => ("index.html", f),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    "<!doctype html><meta charset=utf-8><title>Strata Core</title><body style=\"font:14px system-ui;background:#0b0c12;color:#e6e8f2;padding:40px\">Web app not built: <code>cd web &amp;&amp; npm install &amp;&amp; npm run build</code>",
                )
                    .into_response()
            }
        },
    };
    let mime = file.metadata.mimetype().to_string();
    // Les fichiers à empreinte de Vite (static/) ne changent jamais ; le reste est revérifié.
    let cache = if path.starts_with("static/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    let mut res = Response::new(Body::from(file.data.into_owned()));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    res
}
