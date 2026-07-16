use crate::config::SiteConfig;
use crate::render_page::render_page;
use anyhow::{anyhow, Result};
use notify::RecommendedWatcher;
use notify::{RecursiveMode, Watcher};
use render_core::site::{build_site, SiteModel};
use render_core::LinkPolicy;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
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
    site: &mut SiteModel,
    project_dir: &Path,
    epoch: u64,
) -> HashMap<String, String> {
    // A repo-root CLAUDE.md (outside the docs tree) is surfaced as a nav page.
    crate::render_page::surface_repo_claude(site, cfg, project_dir);
    // A docs tree with no index.md still gets a working `/` (see `resolve_home`).
    let home = crate::render_page::resolve_home(site, cfg, project_dir);
    let order = crate::render_page::reading_order(&site.nav, home.as_ref());
    site.pages
        .iter()
        .chain(home.as_ref())
        .map(|p| {
            let (prev, next) = crate::render_page::neighbours(&order, &p.url);
            (
                p.url.clone(),
                inject_reload(&render_page(cfg, &site.nav, p, prev, next), epoch),
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

fn handle(req: Request, state: &RwLock<ServedSite>, docs: &Path, exclude: &[String]) {
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

    // Embedded shell assets (not in the page map, not on disk under serve).
    if url == crate::assets::CSS_URL {
        respond(
            req,
            200,
            content_type(&url),
            crate::assets::stylesheet().as_bytes().to_vec(),
        );
        return;
    }
    if url == crate::assets::JS_URL {
        respond(
            req,
            200,
            content_type(&url),
            crate::assets::COMPOSITOR_JS.as_bytes().to_vec(),
        );
        return;
    }

    // On-demand asset straight from docs_dir (never .md, never traversing out,
    // never a path the config excludes — `exclude` hides a tree from `build`,
    // and serving it anyway on direct URL would defeat that, especially with
    // `serve --host 0.0.0.0`).
    if is_safe(&url)
        && !url.ends_with(".md")
        && !render_core::exclude::is_excluded(Path::new(&url), exclude)
    {
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
fn rebuild_into(state: &RwLock<ServedSite>, cfg: &SiteConfig, docs: &Path, project_dir: &Path) {
    match build_site(docs, LinkPolicy::Lenient, &cfg.exclude) {
        Ok(mut site) => {
            let next_epoch = state.read().expect("state lock").epoch + 1;
            let pages = build_pages(cfg, &mut site, project_dir, next_epoch);
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
///
/// **The caller owns the returned watcher and must keep it alive** — dropping it drops the
/// notify sender, which ends the rebuild thread. That is the only shutdown path: ownership
/// lives with the caller so a host running many sites can stop one without leaking its
/// thread (see `ServeHandle`). `None` means live-reload is disabled (init or watch failed);
/// serving still works.
fn spawn_watcher(
    state: Arc<RwLock<ServedSite>>,
    cfg: SiteConfig,
    docs: PathBuf,
    project_dir: PathBuf,
) -> Option<(RecommendedWatcher, JoinHandle<()>)> {
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
                return None;
            }
        };
    if let Err(e) = watcher.watch(&docs, RecursiveMode::Recursive) {
        eprintln!(
            "watching {} failed, live-reload disabled: {e}",
            docs.display()
        );
        return None;
    }
    let thread = std::thread::spawn(move || {
        loop {
            // Block until the first event, then drain the quiet window. `recv` errors once
            // the watcher (and with it the sender) is dropped — that is the exit.
            if rx.recv().is_err() {
                break;
            }
            while rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_ok()
            {}
            rebuild_into(&state, &cfg, &docs, &project_dir);
        }
    });
    Some((watcher, thread))
}

pub(crate) fn serve_loop(
    server: &Server,
    state: Arc<RwLock<ServedSite>>,
    docs: PathBuf,
    exclude: Vec<String>,
) {
    for req in server.incoming_requests() {
        handle(req, &state, &docs, &exclude);
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

/// Bind the HTTP server. `None` port binds `:0` so the OS assigns a free
/// ephemeral port (the default — never fails on "address in use"); `Some(p)`
/// binds exactly `p` and errors loudly if it is taken (honoring `--port`).
fn bind_server(host: &str, port: Option<u16>) -> Result<Server> {
    let requested = port.unwrap_or(0);
    Server::http(format!("{host}:{requested}")).map_err(|e| match port {
        Some(p) => anyhow!("binding {host}:{p}: {e}"),
        None => anyhow!("binding {host} on an ephemeral port: {e}"),
    })
}

/// Everything a serve entry point needs, assembled but not yet blocking. Both `run_serve`
/// (CLI, blocks) and `serve_handle` (host apps, returns) build from this — one setup path,
/// so the two entry points cannot drift.
struct Serving {
    server: Server,
    state: Arc<RwLock<ServedSite>>,
    docs: PathBuf,
    exclude: Vec<String>,
    /// `None` = live-reload disabled. The caller must keep this alive; see `spawn_watcher`.
    watcher: Option<(RecommendedWatcher, JoinHandle<()>)>,
}

/// Load the config, build the site, start the watcher, and bind the port — everything up to
/// (but not including) the blocking request loop.
fn setup(project_dir: &Path, host: &str, port: Option<u16>) -> Result<Serving> {
    let cfg = SiteConfig::load(project_dir)?;
    let docs = cfg.docs_path(project_dir);

    let mut site = build_site(&docs, LinkPolicy::Lenient, &cfg.exclude)?;
    let state = Arc::new(RwLock::new(ServedSite {
        pages: build_pages(&cfg, &mut site, project_dir, 0),
        epoch: 0,
    }));

    // Captured before `cfg` moves into `spawn_watcher` — the on-demand asset branch in
    // `handle` needs it too.
    let exclude = cfg.exclude.clone();

    let watcher = spawn_watcher(
        Arc::clone(&state),
        cfg,
        docs.clone(),
        project_dir.to_path_buf(),
    );

    let server = bind_server(host, port)?;
    Ok(Serving {
        server,
        state,
        docs,
        exclude,
        watcher,
    })
}

pub fn run_serve(project_dir: &Path, host: &str, port: Option<u16>, open: bool) -> Result<()> {
    let Serving {
        server,
        state,
        docs,
        exclude,
        // Held until this function returns; dropping it would disable live-reload.
        watcher: _watcher,
    } = setup(project_dir, host, port)?;

    let listen = server.server_addr();
    println!("compositor serving {} on http://{listen}/", docs.display());
    if open {
        open_browser(&format!("http://{listen}/"));
    }
    serve_loop(&server, state, docs, exclude);
    Ok(())
}

/// A running site: bound, serving, and watching — shut down on demand.
///
/// Returned by [`serve_handle`] once the port is bound, so a host app can start a site and
/// immediately point a webview at `port`. Shutdown is idempotent and also runs on drop, so a
/// dropped handle never leaks its threads.
pub struct ServeHandle {
    /// The bound loopback port. Assigned by the OS (`:0`), so it is never "address in use".
    pub port: u16,
    server: Option<Arc<Server>>,
    serve_thread: Option<JoinHandle<()>>,
    watcher: Option<(RecommendedWatcher, JoinHandle<()>)>,
}

impl ServeHandle {
    /// Stop serving and watching, and wait for both threads to exit.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        // Watcher first: dropping it drops the notify sender, which is what ends the rebuild
        // thread (see `spawn_watcher`). There is no other exit.
        if let Some((watcher, thread)) = self.watcher.take() {
            drop(watcher);
            let _ = thread.join();
        }
        // `unblock` ends `incoming_requests`, which returns `serve_loop`.
        if let Some(server) = &self.server {
            server.unblock();
        }
        if let Some(thread) = self.serve_thread.take() {
            let _ = thread.join();
        }
        // Drop our last `Server` reference — everything this handle owns. `take()` also makes
        // a second `stop()` call (reachable from both `shutdown(self)` and `Drop` on the same
        // value) a safe no-op. Note this does *not* guarantee the OS-level port is free the
        // instant this returns: tiny_http's own `Drop for Server` signals its internal accept
        // thread but doesn't wait for it to exit (see the test, which is the one place that
        // postcondition actually matters).
        self.server.take();
    }
}

impl Drop for ServeHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Serve `project_dir` on an OS-assigned loopback port, returning once bound.
///
/// The non-blocking counterpart to [`run_serve`], for hosts embedding many sites in one
/// process. Both build from the same `setup`, so the CLI and the embedded path cannot drift.
pub fn serve_handle(project_dir: &Path) -> Result<ServeHandle> {
    let Serving {
        server,
        state,
        docs,
        exclude,
        watcher,
    } = setup(project_dir, "127.0.0.1", None)?;

    let port = server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .ok_or_else(|| anyhow!("serve bound a non-IP address"))?;

    let server = Arc::new(server);
    let loop_server = Arc::clone(&server);
    let serve_thread = std::thread::spawn(move || serve_loop(&loop_server, state, docs, exclude));

    Ok(ServeHandle {
        port,
        server: Some(server),
        serve_thread: Some(serve_thread),
        watcher,
    })
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
    fn bind_server_none_picks_a_free_ephemeral_port() {
        // No --port: the OS assigns a free port, so the bound address must be
        // non-zero and reachable.
        let server = bind_server("127.0.0.1", None).expect("ephemeral bind");
        let port = server.server_addr().to_ip().unwrap().port();
        assert_ne!(port, 0, "ephemeral bind must resolve to a real port");
    }

    #[test]
    fn bind_server_explicit_taken_port_errors() {
        // --port N honors intent: if N is already taken, binding it again is a
        // hard error rather than silently falling back to another port.
        let held = bind_server("127.0.0.1", None).expect("hold a port");
        let taken = held.server_addr().to_ip().unwrap().port();
        let err = bind_server("127.0.0.1", Some(taken));
        assert!(err.is_err(), "rebinding a taken explicit port must error");
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
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let state = sample_state();
        let docs = std::env::temp_dir();
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs, vec![]));

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
    fn serves_embedded_shell_css() {
        let state = sample_state();
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let docs = std::path::PathBuf::from(".");
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs, vec![]));
        let css = get(addr, "/assets/compositor.css");
        assert!(css.contains(".topbar"));
    }

    #[test]
    fn serves_embedded_shell_js() {
        let state = sample_state();
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let docs = std::path::PathBuf::from(".");
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs, vec![]));
        let js = get(addr, "/assets/compositor.js");
        assert!(js.contains("addEventListener"));
    }

    #[test]
    fn on_demand_asset_honors_exclude() {
        // An asset under an excluded prefix must not be servable by direct
        // URL, even though it sits right on disk under `docs` — the whole
        // point of `exclude` is to hide a tree, and the on-demand asset
        // branch used to bypass it entirely (a network exposure under
        // `serve --host 0.0.0.0`). A kept, non-excluded asset must still
        // serve normally.
        let tmp =
            std::env::temp_dir().join(format!("compositor-serve-excl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("superpowers")).unwrap();
        std::fs::create_dir_all(tmp.join("guides")).unwrap();
        std::fs::write(tmp.join("superpowers/note.txt"), "secret").unwrap();
        std::fs::write(tmp.join("guides/kept.txt"), "public").unwrap();

        let state = sample_state();
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let exclude = vec!["superpowers/".to_string()];
        let docs_for_thread = tmp.clone();
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs_for_thread, exclude));

        let excluded = get(addr, "/superpowers/note.txt");
        assert!(
            excluded.contains("404"),
            "excluded asset must not be served: {excluded}"
        );

        let kept = get(addr, "/guides/kept.txt");
        assert!(kept.contains("200 OK"), "kept asset resp: {kept}");
        assert!(kept.contains("public"), "kept asset resp: {kept}");

        std::fs::remove_dir_all(&tmp).ok();
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
        let mut site = build_site(&docs, LinkPolicy::Lenient, &[]).unwrap();
        let state = RwLock::new(ServedSite {
            pages: build_pages(&cfg, &mut site, &tmp, 0),
            epoch: 0,
        });
        assert!(state.read().unwrap().pages["index.html"].contains("One"));

        // A change lands; one rebuild must swap content and advance the epoch.
        std::fs::write(docs.join("index.md"), "# Two").unwrap();
        rebuild_into(&state, &cfg, &docs, &tmp);

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

    #[test]
    fn dropping_the_watcher_ends_the_rebuild_thread() {
        // The rebuild thread must terminate when its watcher is dropped. If it doesn't,
        // a host app that stops a site leaks a thread + an inotify/FSEvents handle per stop.
        // Failure mode is a hang on join(), not an assert — that is the bug.
        let tmp = std::env::temp_dir().join(format!("compositor-watcher-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        std::fs::write(tmp.join("docs/index.md"), "# Hi\n").unwrap();

        let cfg = SiteConfig::load(&tmp).unwrap();
        let docs = cfg.docs_path(&tmp);
        let (watcher, thread) =
            spawn_watcher(sample_state(), cfg, docs, tmp.clone()).expect("watcher starts");

        drop(watcher);
        thread
            .join()
            .expect("rebuild thread must exit once its watcher is dropped");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn serve_handle_serves_then_releases_the_port() {
        let tmp = std::env::temp_dir().join(format!("compositor-handle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        std::fs::write(tmp.join("docs/index.md"), "# Handle\n").unwrap();

        let h = serve_handle(&tmp).expect("serve_handle binds");
        let port = h.port;
        assert!(port > 0, "an ephemeral port must be reported");

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let page = get(addr, "/");
        assert!(page.contains("200 OK"), "page resp: {page}");
        assert!(
            page.contains("/__reload"),
            "live-reload must be injected: {page}"
        );

        h.shutdown();

        // This proves `ServeHandle` released its `Server` — the leak this plan exists to
        // prevent. (It does *not* prove the serve thread exited; `shutdown()`'s internal
        // `serve_thread.join()` already does that, and the serve thread never held the
        // listener in the first place — tiny_http's own internal accept thread does.)
        //
        // That accept thread is why this needs a retry rather than a single bind: `Drop for
        // Server` signals the accept thread and self-connects to unblock its blocking
        // `accept()` call, then returns immediately — it does not wait for that thread to
        // observe the signal, break its loop, and actually drop the `TcpListener`, which is
        // what releases the OS-level port. So the port's release is asynchronous relative to
        // `shutdown()` returning. Retrying the bind *is* the fix: the retry and the assertion
        // are the same operation, so there is no separate probe-then-assert gap for another
        // test (this file binds `127.0.0.1:0` in several places, running in parallel) to steal
        // the port in between. Bounded, not indefinite — a hang here is a real regression, not
        // something to paper over with a longer sleep.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let rebound = loop {
            match tiny_http::Server::http(("127.0.0.1", port)) {
                Ok(s) => break Some(s),
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => break None,
            }
        };
        assert!(rebound.is_some(), "port {port} still bound after shutdown");

        std::fs::remove_dir_all(&tmp).ok();
    }
}
