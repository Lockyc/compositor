use crate::config::SiteConfig;
use crate::render_page::render_page;
use anyhow::{anyhow, Result};
use notify::{RecursiveMode, Watcher};
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

/// The client script, baselined at the epoch the page was built at (not the
/// first `/__reload` poll response) — otherwise a reload that lands between
/// page load and the first poll gets swallowed: see module docs.
fn reload_script(epoch: u64) -> String {
    format!(
        r#"<script>
(function () {{
  var e = "{epoch}";
  setInterval(function () {{
    fetch('/__reload').then(function (r) {{ return r.text(); }}).then(function (t) {{
      if (t !== e) {{ location.reload(); }}
    }}).catch(function () {{}});
  }}, 250);
}})();
</script>"#
    )
}

fn inject_reload(html: &str, epoch: u64) -> String {
    let script = reload_script(epoch);
    match html.rfind("</body>") {
        Some(i) => format!("{}{}{}", &html[..i], script, &html[i..]),
        None => format!("{html}{script}"),
    }
}

/// Render every page and inject the reload script — the map the server sends.
/// `epoch` is baked into each page as the client's baseline, so it must equal
/// the epoch `/__reload` will report for this build (see `rebuild_into`).
pub(crate) fn build_pages(
    cfg: &SiteConfig,
    site: &SiteModel,
    epoch: u64,
) -> HashMap<String, String> {
    site.pages
        .iter()
        .map(|p| {
            (
                p.url.clone(),
                inject_reload(&render_page(cfg, &site.nav, p), epoch),
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
    let ctype_header = Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes())
        .expect("static content-type header is valid");
    // A live-reload server must never serve stale content: the `/__reload`
    // liveness poll especially must never be cached by a browser (fetch's
    // heuristic cache) or an interposed proxy, or the client polls a frozen
    // epoch and reload silently stops — and an edited asset would reload stale.
    // `no-store` on every response keeps "save and see it" honest.
    let cache_header = Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..])
        .expect("static cache-control header is valid");
    let resp = Response::from_data(body)
        .with_status_code(status)
        .with_header(ctype_header)
        .with_header(cache_header);
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

/// One rebuild cycle: re-render leniently, swap the page map, bump the epoch.
/// A content error is impossible under the lenient policy; an IO race (a file
/// deleted between walk and read) logs and leaves the last-good map live — the
/// next filesystem event retries. Never crashes, never swallows a good build.
///
/// `build_pages` (the expensive render work) runs outside the write lock —
/// only the single watcher thread ever calls this, so read-then-write is
/// race-free — and the write-guard critical section is limited to the two
/// assignments so a panic during rendering can't poison the lock.
fn rebuild_into(state: &RwLock<ServedSite>, cfg: &SiteConfig, docs: &Path) {
    match build_site(docs, LinkPolicy::Lenient) {
        Ok(site) => {
            let next_epoch = state.read().expect("state lock").epoch + 1;
            let pages = build_pages(cfg, &site, next_epoch);
            let mut s = state.write().expect("state lock");
            s.pages = pages;
            s.epoch = next_epoch;
        }
        Err(e) => eprintln!("rebuild failed, keeping last good site: {e:#}"),
    }
}

/// Watch `docs` and rebuild on change. A burst of editor saves is debounced
/// into a single rebuild by draining the event channel over a ~200ms quiet
/// window before rebuilding.
fn spawn_watcher(state: Arc<RwLock<ServedSite>>, cfg: SiteConfig, docs: PathBuf) {
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher =
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = tx.send(());
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("file watcher init failed, live-reload disabled: {e}");
                    return;
                }
            };
        if let Err(e) = watcher.watch(&docs, RecursiveMode::Recursive) {
            eprintln!(
                "watching {} failed, live-reload disabled: {e}",
                docs.display()
            );
            return;
        }
        loop {
            // Block until the first event, then drain the quiet window.
            if rx.recv().is_err() {
                break;
            }
            while rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_ok()
            {}
            rebuild_into(&state, &cfg, &docs);
        }
    });
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
    let cfg = SiteConfig::load(project_dir)?;
    let docs = cfg.docs_path(project_dir);

    let site = build_site(&docs, LinkPolicy::Lenient)?;
    let state = Arc::new(RwLock::new(ServedSite {
        pages: build_pages(&cfg, &site, 0),
        epoch: 0,
    }));

    spawn_watcher(Arc::clone(&state), cfg, docs.clone());

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
        let out = inject_reload("<body>hi</body>", 0);
        assert!(out.contains("/__reload"));
        assert!(out.contains("hi"));
        // The script is spliced in immediately before "</body>", so the
        // original closing tag is untouched and still ends the output.
        assert!(out.ends_with("</body>"));
        assert_eq!(out.matches("/__reload").count(), 1);
    }

    #[test]
    fn inject_reload_bakes_the_build_epoch_as_baseline() {
        // The baseline must be the page's build epoch, not "unset" — a stale
        // first-poll baseline (the bug this guards against) would swallow an
        // edit that lands between page load and the first /__reload poll.
        let out = inject_reload("<body>hi</body>", 7);
        assert!(out.contains(r#"var e = "7""#), "script: {out}");
        assert!(
            !out.contains("=== null"),
            "no first-poll baseline logic: {out}"
        );
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
            inject_reload("<body>Hello</body>", 0),
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
        // Live content must be uncacheable, or a browser/proxy cache defeats
        // live-reload (see `respond`).
        assert!(
            page.to_lowercase().contains("cache-control: no-store"),
            "page resp: {page}"
        );

        let reload = get(addr, "/__reload");
        assert!(reload.contains("200 OK"));
        assert!(reload.trim_end().ends_with('0')); // epoch 0
                                                   // The liveness poll especially must never be cached.
        assert!(
            reload.to_lowercase().contains("cache-control: no-store"),
            "reload resp: {reload}"
        );
    }

    #[test]
    fn rebuild_bumps_epoch_and_swaps_content() {
        let tmp = std::env::temp_dir().join(format!("compositor-serve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("index.md"), "# One").unwrap();

        let cfg = SiteConfig {
            site_name: "T".to_string(),
            docs_dir: Some("docs".to_string()),
            ..Default::default()
        };
        let site = build_site(&docs, LinkPolicy::Lenient).unwrap();
        let state = RwLock::new(ServedSite {
            pages: build_pages(&cfg, &site, 0),
            epoch: 0,
        });
        assert!(state.read().unwrap().pages["index.html"].contains("One"));

        // A change lands; one rebuild must swap content and advance the epoch.
        std::fs::write(docs.join("index.md"), "# Two").unwrap();
        rebuild_into(&state, &cfg, &docs);

        {
            let s = state.read().unwrap();
            assert_eq!(s.epoch, 1);
            assert!(s.pages["index.html"].contains("Two"));
            assert!(!s.pages["index.html"].contains("One"));
            // The served page's baked baseline must agree with the new epoch
            // that /__reload will report — this is the invariant the bug
            // broke (baseline from the first poll, not the build epoch).
            assert!(
                s.pages["index.html"].contains(r#"var e = "1""#),
                "page: {}",
                s.pages["index.html"]
            );
        }
        std::fs::remove_dir_all(&tmp).ok();
    }
}
