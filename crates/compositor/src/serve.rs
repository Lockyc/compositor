use crate::config::SiteConfig;
use crate::render_page::render_page;
use anyhow::{anyhow, Result};
use render_core::site::{build_site, SiteModel};
use render_core::LinkPolicy;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use tiny_http::{Header, Request, Response, Server};

/// The live-reloaded site: url ("cli/tar.html") -> ready-to-send HTML (reload
/// script already injected), plus a monotonic build counter the browser polls.
pub(crate) struct ServedSite {
    pub(crate) pages: HashMap<String, String>,
    pub(crate) epoch: u64,
}

const RELOAD_SCRIPT: &str = r#"<script>
(function () {
  var e = null;
  setInterval(function () {
    fetch('/__reload').then(function (r) { return r.text(); }).then(function (t) {
      if (e === null) { e = t; return; }
      if (t !== e) { location.reload(); }
    }).catch(function () {});
  }, 250);
})();
</script>"#;

fn inject_reload(html: &str) -> String {
    match html.rfind("</body>") {
        Some(i) => format!("{}{}{}", &html[..i], RELOAD_SCRIPT, &html[i..]),
        None => format!("{html}{RELOAD_SCRIPT}"),
    }
}

/// Render every page and inject the reload script — the map the server sends.
pub(crate) fn build_pages(cfg: &SiteConfig, site: &SiteModel) -> HashMap<String, String> {
    site.pages
        .iter()
        .map(|p| {
            (
                p.url.clone(),
                inject_reload(&render_page(cfg, &site.nav, p)),
            )
        })
        .collect()
}

/// Map a request path to a site url: "" / "/" -> index.html, trailing "/" ->
/// append index.html, else the path with its leading slash stripped.
fn request_url(raw_path: &str) -> String {
    let trimmed = raw_path.trim_start_matches('/');
    if trimmed.is_empty() {
        "index.html".to_string()
    } else if trimmed.ends_with('/') {
        format!("{trimmed}index.html")
    } else {
        trimmed.to_string()
    }
}

/// Reject any url that could escape the docs dir. Only the on-disk asset branch
/// uses the raw url against the filesystem; page lookups hit a known-key map.
fn is_safe(url: &str) -> bool {
    Path::new(url)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn respond(req: Request, status: u16, ctype: &str, body: Vec<u8>) {
    let header = Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes())
        .expect("static content-type header is valid");
    let resp = Response::from_data(body)
        .with_status_code(status)
        .with_header(header);
    let _ = req.respond(resp);
}

fn handle(req: Request, state: &RwLock<ServedSite>, docs: &Path) {
    let raw = req.url().to_string();
    let path_only = raw.split(['?', '#']).next().unwrap_or("");

    if path_only == "/__reload" {
        let epoch = state.read().expect("state lock").epoch;
        respond(
            req,
            200,
            "text/plain; charset=utf-8",
            epoch.to_string().into_bytes(),
        );
        return;
    }

    let url = request_url(path_only);

    let page = state.read().expect("state lock").pages.get(&url).cloned();
    if let Some(html) = page {
        respond(req, 200, "text/html; charset=utf-8", html.into_bytes());
        return;
    }

    // On-demand asset straight from docs_dir (never .md, never traversing out).
    if is_safe(&url) && !url.ends_with(".md") {
        let asset = docs.join(&url);
        if asset.is_file() {
            if let Ok(bytes) = std::fs::read(&asset) {
                respond(req, 200, content_type(&url), bytes);
                return;
            }
        }
    }

    respond(
        req,
        404,
        "text/plain; charset=utf-8",
        b"404 Not Found".to_vec(),
    );
}

pub(crate) fn serve_loop(server: Server, state: Arc<RwLock<ServedSite>>, docs: PathBuf) {
    for req in server.incoming_requests() {
        handle(req, &state, &docs);
    }
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(url).spawn();
}

pub fn run_serve(project_dir: &Path, host: &str, port: u16, open: bool) -> Result<()> {
    let cfg_path = project_dir.join("compositor.toml");
    let cfg: SiteConfig = toml::from_str(&std::fs::read_to_string(&cfg_path)?)?;
    let docs = project_dir.join(cfg.docs_dir());

    let site = build_site(&docs, LinkPolicy::Lenient)?;
    let state = Arc::new(RwLock::new(ServedSite {
        pages: build_pages(&cfg, &site),
        epoch: 0,
    }));

    // Task 3 wires the watcher here:
    // spawn_watcher(Arc::clone(&state), cfg, docs.clone());

    let server = Server::http(format!("{host}:{port}"))
        .map_err(|e| anyhow!("binding {host}:{port}: {e}"))?;
    let listen = server.server_addr();
    println!("compositor serving {} on http://{listen}/", docs.display());
    if open {
        open_browser(&format!("http://{listen}/"));
    }
    serve_loop(server, state, docs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;

    #[test]
    fn request_url_normalizes_paths() {
        assert_eq!(request_url("/"), "index.html");
        assert_eq!(request_url(""), "index.html");
        assert_eq!(request_url("/cli/tar.html"), "cli/tar.html");
        assert_eq!(request_url("/cli/"), "cli/index.html");
    }

    #[test]
    fn content_type_by_extension() {
        assert_eq!(content_type("a.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("a.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("a.png"), "image/png");
        assert_eq!(content_type("a.unknown"), "application/octet-stream");
    }

    #[test]
    fn inject_reload_inserts_once_before_body_close() {
        let out = inject_reload("<body>hi</body>");
        assert!(out.contains("/__reload"));
        assert!(out.contains("hi"));
        // The script is spliced in immediately before "</body>", so the
        // original closing tag is untouched and still ends the output.
        assert!(out.ends_with("</body>"));
        assert_eq!(out.matches("/__reload").count(), 1);
    }

    #[test]
    fn is_safe_rejects_traversal() {
        assert!(is_safe("cli/tar.html"));
        assert!(!is_safe("../secret"));
        assert!(!is_safe("a/../../b"));
    }

    fn sample_state() -> Arc<RwLock<ServedSite>> {
        let mut pages = HashMap::new();
        pages.insert(
            "index.html".to_string(),
            inject_reload("<body>Hello</body>"),
        );
        Arc::new(RwLock::new(ServedSite { pages, epoch: 0 }))
    }

    fn get(addr: std::net::SocketAddr, path: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        resp
    }

    #[test]
    fn serves_page_and_reload_endpoint() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        let state = sample_state();
        let docs = std::env::temp_dir();
        std::thread::spawn(move || serve_loop(server, state, docs));

        let page = get(addr, "/");
        assert!(page.contains("200 OK"), "page resp: {page}");
        assert!(page.contains("Hello"));
        assert!(page.contains("/__reload"));

        let reload = get(addr, "/__reload");
        assert!(reload.contains("200 OK"));
        assert!(reload.trim_end().ends_with('0')); // epoch 0
    }
}
