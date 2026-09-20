//! Untrusted documents use an opaque, sandboxed inner frame. A trusted outer
//! frame's response policy blocks inner-frame navigations to remote origins.
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::http::{Request, Response};

const CONTENT_CSP:&str="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; sandbox allow-scripts";
const FRAME_CSP:&str="default-src 'none'; style-src 'unsafe-inline'; frame-src fox-preview: http://fox-preview.localhost https://fox-preview.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; sandbox allow-scripts";
struct Document {
    owner: String,
    content: String,
    created: Instant,
}
fn documents() -> &'static Mutex<HashMap<String, Document>> {
    static DOCS: OnceLock<Mutex<HashMap<String, Document>>> = OnceLock::new();
    DOCS.get_or_init(Default::default)
}
fn insert(owner: &str, content: String) -> Result<String, String> {
    if content.len() > 4 * 1024 * 1024 {
        return Err("网页预览上限为 4 MiB，请查看源码。".into());
    }
    let mut docs = documents()
        .lock()
        .map_err(|_| "Preview store unavailable")?;
    docs.retain(|_, d| d.created.elapsed() < Duration::from_secs(3600));
    if docs.len() >= 32
        || docs.values().map(|d| d.content.len()).sum::<usize>() + content.len() > 16 * 1024 * 1024
    {
        return Err("请先关闭其他网页预览。".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    docs.insert(
        id.clone(),
        Document {
            owner: owner.into(),
            content,
            created: Instant::now(),
        },
    );
    Ok(id)
}
#[tauri::command]
pub(crate) fn html_preview_create(
    window: tauri::WebviewWindow,
    content: String,
) -> Result<String, String> {
    insert(window.label(), content)
}
#[tauri::command]
pub(crate) fn html_preview_release(window: tauri::WebviewWindow, id: String) -> Result<(), String> {
    let mut docs = documents()
        .lock()
        .map_err(|_| "Preview store unavailable")?;
    if docs.get(&id).is_some_and(|d| d.owner == window.label()) {
        docs.remove(&id);
    }
    Ok(())
}
pub(crate) fn serve(owner: &str, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let (status, body, csp) = (|| {
        if request.method() != "GET" || request.uri().query().is_some() {
            return (405, String::new(), FRAME_CSP);
        }
        let Some((id, kind)) = request.uri().path().trim_start_matches('/').split_once('/') else {
            return (404, String::new(), FRAME_CSP);
        };
        let Ok(docs) = documents().lock() else {
            return (503, String::new(), FRAME_CSP);
        };
        let Some(doc) = docs
            .get(id)
            .filter(|d| d.owner == owner && d.created.elapsed() < Duration::from_secs(3600))
        else {
            return (404, String::new(), FRAME_CSP);
        };
        match kind {
            "frame" => (200,format!("<!doctype html><meta charset=utf-8><style>html,body,iframe{{margin:0;border:0;width:100%;height:100%;display:block}}</style><iframe title=\"网页内容\" sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" src=\"/{id}/content\"></iframe>"),FRAME_CSP),
            "content" => (200,doc.content.clone(),CONTENT_CSP),
            _ => (404,String::new(),FRAME_CSP),
        }
    })();
    Response::builder()
        .status(status)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Security-Policy", csp)
        .header("Cache-Control", "no-store")
        .header("Referrer-Policy", "no-referrer")
        .header("X-Content-Type-Options", "nosniff")
        .header(
            "Permissions-Policy",
            "camera=(), microphone=(), geolocation=(), clipboard-read=(), clipboard-write=()",
        )
        .body(body.into_bytes())
        .expect("static preview response headers")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_keeps_interaction_but_separates_origin_and_navigation() {
        let id=insert("preview-test","<button onclick=\"this.textContent='ok'\">Go</button><script>document.title='ready'</script>".into()).unwrap();
        let req = |kind| {
            Request::builder()
                .uri(format!("http://fox-preview.localhost/{id}/{kind}"))
                .body(vec![])
                .unwrap()
        };
        let frame = serve("preview-test", req("frame"));
        assert_eq!(frame.status(), 200);
        assert!(frame.headers()["Content-Security-Policy"]
            .to_str()
            .unwrap()
            .contains("frame-src fox-preview:"));
        let content = serve("preview-test", req("content"));
        assert_eq!(content.status(), 200);
        assert!(content.headers()["Content-Security-Policy"]
            .to_str()
            .unwrap()
            .contains("connect-src 'none'"));
        assert!(!content.headers()["Content-Security-Policy"]
            .to_str()
            .unwrap()
            .contains("allow-same-origin"));
        assert!(String::from_utf8(content.into_body())
            .unwrap()
            .contains("onclick"));
        assert_eq!(serve("different-window", req("content")).status(), 404);
        assert_eq!(serve("preview-test", req("../content")).status(), 404);
    }
}
