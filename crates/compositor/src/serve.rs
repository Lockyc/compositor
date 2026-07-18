use crate::config::SiteConfig;
use crate::render_page::render_page;
use anyhow::{anyhow, Result};
use notify::RecommendedWatcher;
use notify::{RecursiveMode, Watcher};
use render_core::site::{build_site, SiteModel};
use render_core::{Excluder, LinkPolicy};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use tiny_http::{Header, Request, Response, Server};

/// The live-reloaded site: url ("cli/tar.html") -> ready-to-send HTML (reload
/// script already injected), plus a monotonic build counter the browser polls.
///
/// The `Excluder` lives here, not captured alongside, so `handle`'s on-demand
/// asset check reads the same rules the live render used. Captured separately it
/// would silently drift the moment a rebuild picked up a `.gitignore` edit.
pub(crate) struct ServedSite {
    pub(crate) pages: HashMap<String, String>,
    pub(crate) epoch: u64,
    pub(crate) excluder: Arc<Excluder>,
    /// site url -> source file, for images the repo-root pages reference from
    /// outside the docs tree. `handle` cannot find these under `docs`, and
    /// rebuilding it alongside `pages` is what keeps the two from drifting.
    pub(crate) root_assets: HashMap<String, PathBuf>,
    /// site url -> the on-disk Markdown file that url writes to. Empty
    /// whenever `edit_enabled` is false (never populated by `build_pages`
    /// in that mode) — a later write endpoint must never trust a url that
    /// isn't in this map.
    pub(crate) editable: HashMap<String, PathBuf>,
    /// Whether this site is serving with inline editing on — decided once in
    /// `setup` from the bind host (see `host_is_loopback`) and carried
    /// unchanged through every rebuild.
    pub(crate) edit_enabled: bool,
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

/// One page's edit-mode payload: the client's `#__editsrc` script tag decodes
/// this to map a rendered block back to a source line range for autosave (see
/// `EditSource`). Field names are the client's JS-camelCase contract, not
/// Rust's — `serde(rename)` is the single source for that mismatch.
#[derive(serde::Serialize)]
struct EditPayload<'a> {
    url: &'a str,
    source: &'a str,
    #[serde(rename = "fmLines")]
    fm_lines: usize,
    #[serde(rename = "lineMap")]
    line_map: &'a [Option<usize>],
}

/// Inject the edit-mode scaffolding into one already-rendered page: the editor
/// stylesheet, the toggle button, and the page's payload + client script.
/// Mirrors `inject_reload`'s splice-before-marker approach (`rfind`, degrade
/// to appending if the marker is missing — a rendered page always has all
/// three, but this must never panic on a hand-crafted or future template).
/// Call this *after* `inject_reload` so the reload script and the editor
/// script both land before `</body>`, reload first.
fn inject_editor(html: &str, payload_json: &str, asset_prefix: &str) -> String {
    let css = format!(r#"<link rel="stylesheet" href="{asset_prefix}assets/editor.css">"#);
    let out = match html.rfind("</head>") {
        Some(i) => format!("{}{}{}", &html[..i], css, &html[i..]),
        None => format!("{html}{css}"),
    };

    let toggle = r#"<button type="button" class="edit-toggle">Edit</button>"#;
    let out = match out.rfind("</header>") {
        Some(i) => format!("{}{}{}", &out[..i], toggle, &out[i..]),
        None => format!("{out}{toggle}"),
    };

    // The payload embeds the page's verbatim Markdown source inside a
    // `<script>` tag: a source file containing the literal text "</script"
    // (in a fenced code block, say) would otherwise close the tag early and
    // corrupt the page. The standard mitigation: escape every "</" the JSON
    // serializer could have emitted inside a string so no substring can ever
    // match a tag-closing sequence.
    let safe_json = payload_json.replace("</", "<\\/");
    let scripts = format!(
        r#"<script type="application/json" id="__editsrc">{safe_json}</script><script src="{asset_prefix}assets/editor.js" defer></script>"#
    );
    match out.rfind("</body>") {
        Some(i) => format!("{}{}{}", &out[..i], scripts, &out[i..]),
        None => format!("{out}{scripts}"),
    }
}

/// A docs-tree page at the root of the tree named `index`/`home`/`readme`
/// (case-insensitive) — the same tier-1/2 predicate `resolve_home` uses
/// internally to decide whether it promotes a docs-tree page rather than
/// falling through to the repo-root README. Duplicated narrowly here (rather
/// than exposed from `render_page`) so `build_pages` can tell whether a
/// synthetic home entry aliases an existing, already-mapped docs page — in
/// which case its bogus placeholder `rel_path` ("index.md") must not be
/// resolved as if it were real.
fn has_docs_root_candidate(site: &SiteModel) -> bool {
    site.pages.iter().any(|p| {
        p.rel_path.parent().is_none_or(|d| d.as_os_str().is_empty())
            && p.rel_path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| {
                    matches!(s.to_ascii_lowercase().as_str(), "index" | "home" | "readme")
                })
    })
}

/// The repo-root `README.md` (any case), if any — the one home-resolution
/// tier the brief asks `build_pages` to map without re-deriving
/// `resolve_home`'s full private tier order. Only meaningful when
/// `!has_docs_root_candidate(site)`; see that function.
fn repo_root_readme(project_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(project_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_file()
                && p.extension().and_then(|e| e.to_str()) == Some("md")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("readme"))
        })
}

/// Render every page, injecting the reload script and — when `edit_enabled`
/// — the editor scaffolding. `epoch` is baked into each page as the client's
/// baseline, so it must equal the epoch `/__reload` will report for this
/// build (see `rebuild_into`).
///
/// Also returns the `editable` map (site url -> on-disk source file), built
/// alongside the render so it can never drift from what was actually served.
/// Population is deliberately conservative: a normal docs page's source
/// always exists on disk by construction, but the promoted-home and
/// surfaced-CLAUDE pages carry a `rel_path` that may not point at a real
/// file (the promoted home's is a bare placeholder), so those only earn an
/// entry when a real backing file is actually resolved — see
/// `has_docs_root_candidate` / `repo_root_readme`. A page with no resolvable
/// source (the generated index; a docs-tree-promoted home aliased under a
/// second url) is simply absent from the map, not an error.
#[allow(clippy::type_complexity)]
pub(crate) fn build_pages(
    cfg: &SiteConfig,
    site: &mut SiteModel,
    project_dir: &Path,
    epoch: u64,
    excluder: &Excluder,
    edit_enabled: bool,
) -> Result<(
    HashMap<String, String>,
    HashMap<String, PathBuf>,
    HashMap<String, PathBuf>,
)> {
    let docs = cfg.docs_path(project_dir);
    // Always lenient: `serve` must never halt an unattended rebuild, so a dead
    // image degrades to a 404 exactly as a dead link does.
    let images =
        crate::root_assets::RootAssets::new(project_dir, &docs, excluder, LinkPolicy::Lenient);
    // A repo-root CLAUDE.md (outside the docs tree) is surfaced as a nav page.
    crate::render_page::surface_repo_claude(site, cfg, project_dir, &images)?;
    // A docs tree with no index.md still gets a working `/` (see `resolve_home`).
    let home = crate::render_page::resolve_home(site, cfg, project_dir, &images)?;
    let order = crate::render_page::reading_order(&site.nav, home.as_ref());

    let mut editable = HashMap::new();
    if edit_enabled {
        // Every normal `site.pages` entry (docs pages and the surfaced
        // CLAUDE.html, if any) has a `rel_path` that is real relative to
        // *either* the docs dir or the project root — try both, first hit
        // wins, so a repo-root-surfaced page resolves without needing to
        // know which case produced it.
        for p in &site.pages {
            let candidate = docs.join(&p.rel_path);
            let src = if candidate.is_file() {
                Some(candidate)
            } else {
                let root_candidate = project_dir.join(&p.rel_path);
                root_candidate.is_file().then_some(root_candidate)
            };
            if let Some(src) = src {
                editable.insert(p.url.clone(), src);
            }
        }
        // The synthetic home page's own `rel_path` is a placeholder, so it is
        // resolved separately: only the repo-root-README tier is mapped here
        // (see `repo_root_readme`'s doc comment for why the docs-promoted
        // tier is left unmapped under `index.html` — its original url
        // already covers the same file via the loop above).
        if let Some(h) = &home {
            if !has_docs_root_candidate(site) {
                if let Some(readme) = repo_root_readme(project_dir) {
                    editable.insert(h.url.clone(), readme);
                }
            }
        }
    }

    let pages = site
        .pages
        .iter()
        .chain(home.as_ref())
        .map(|p| {
            let (prev, next) = crate::render_page::neighbours(&order, &p.url);
            let rendered = inject_reload(&render_page(cfg, &site.nav, p, prev, next), epoch);
            let rendered = if edit_enabled {
                if let Some(es) = &p.edit_source {
                    let depth = p.url.matches('/').count();
                    let asset_prefix = "../".repeat(depth);
                    let payload = EditPayload {
                        url: &p.url,
                        source: &es.source,
                        fm_lines: es.fm_lines,
                        line_map: &es.line_map,
                    };
                    let payload_json = serde_json::to_string(&payload)
                        .expect("EditPayload has no non-serializable field");
                    inject_editor(&rendered, &payload_json, &asset_prefix)
                } else {
                    rendered
                }
            } else {
                rendered
            };
            (p.url.clone(), rendered)
        })
        .collect();
    Ok((pages, images.copies().into_iter().collect(), editable))
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

/// Body of a `POST /__edit` request: the site url being edited and its full
/// replacement Markdown source.
#[derive(serde::Deserialize)]
struct EditRequest {
    url: String,
    source: String,
}

/// A generous but bounded cap on the `/__edit` request body — large enough for
/// any real Markdown page, small enough that a hostile or buggy client can't
/// make this endpoint buffer an unbounded amount of memory.
const MAX_EDIT_BODY: u64 = 10 * 1024 * 1024;

fn respond_text(req: Request, status: u16, msg: &str) {
    respond(
        req,
        status,
        "text/plain; charset=utf-8",
        msg.as_bytes().to_vec(),
    );
}

/// Handle `POST /__edit`: persist an edited page's Markdown to disk.
///
/// `state.editable` is the sole authority for what a url may write to (see
/// its doc comment) — it only ever holds urls resolved to a real, existing
/// backing file. This still re-checks `is_safe` on the *client-supplied* url
/// as defense-in-depth: the write path itself always comes from the map
/// (never from the client), so a client can never name an arbitrary
/// filesystem path, only ask "write to the url you already mapped."
///
/// The write is atomic: the new content lands in a temp sibling
/// (`<path>.md.tmp-<pid>`) first, then an atomic rename replaces the target.
/// A failure between those two steps (full disk, permissions) leaves the
/// last-good file on disk untouched and may leave the temp file behind —
/// harmless scratch, never a partially-written page — and is logged rather
/// than panicking, so the serve loop stays up.
fn handle_edit(mut req: Request, state: &RwLock<ServedSite>) {
    let mut body = Vec::new();
    let read = req
        .as_reader()
        .take(MAX_EDIT_BODY + 1)
        .read_to_end(&mut body);
    if read.is_err() || body.len() as u64 > MAX_EDIT_BODY {
        respond_text(req, 400, "bad request body");
        return;
    }

    let edit: EditRequest = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(_) => {
            respond_text(req, 400, "malformed edit request");
            return;
        }
    };

    let mapped = state
        .read()
        .expect("state lock")
        .editable
        .get(&edit.url)
        .cloned();
    let path = match mapped {
        Some(p) if is_safe(&edit.url) => p,
        _ => {
            respond_text(req, 403, "not an editable target");
            return;
        }
    };

    let tmp_path = path.with_extension(format!("md.tmp-{}", std::process::id()));
    let result =
        std::fs::write(&tmp_path, &edit.source).and_then(|()| std::fs::rename(&tmp_path, &path));
    match result {
        Ok(()) => respond_text(req, 200, "ok"),
        Err(e) => {
            eprintln!("edit write to {} failed: {e}", path.display());
            let _ = std::fs::remove_file(&tmp_path);
            respond_text(req, 500, "write failed");
        }
    }
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

    // The one write path the browser can reach — only live at all when
    // `edit_enabled` (loopback-only, decided once in `setup`). When disabled
    // the endpoint must not exist off loopback, so it falls through to the
    // ordinary page lookup below, which 404s for an unmapped url like this one.
    if path_only == "/__edit" {
        let enabled = state.read().expect("state lock").edit_enabled;
        if enabled {
            handle_edit(req, state);
            return;
        }
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
    // never a path the `Excluder` hides — both `exclude` and gitignore rules hide
    // a tree from `build`, and serving it anyway on direct URL would defeat that,
    // especially with `serve --host 0.0.0.0`).
    let excluder = Arc::clone(&state.read().expect("state lock").excluder);
    if is_safe(&url) && !url.ends_with(".md") && !excluder.is_excluded(Path::new(&url)) {
        let asset = docs.join(&url);
        if asset.is_file() {
            if let Ok(bytes) = std::fs::read(&asset) {
                respond(req, 200, content_type(&url), bytes);
                return;
            }
        }
    }

    // An image a repo-root README/CLAUDE.md referenced from outside the docs
    // tree. Only resolved, non-excluded files are ever in this map (see
    // `RootAssets`), so serving from it is safe by construction.
    let root_src = state
        .read()
        .expect("state lock")
        .root_assets
        .get(&url)
        .cloned();
    if let Some(src) = root_src {
        if let Ok(bytes) = std::fs::read(&src) {
            respond(req, 200, content_type(&url), bytes);
            return;
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
    // Rebuilt per cycle so a `.gitignore` edit takes effect, and stored into the
    // state alongside the pages it produced so the render and `handle`'s
    // on-demand asset check always agree.
    let excluder = Arc::new(Excluder::new(project_dir, docs, &cfg.exclude));
    for w in excluder.warnings() {
        eprintln!("warning: {w}");
    }
    // The bind host never changes mid-session, so `edit_enabled` (decided
    // once in `setup`) is read back rather than recomputed here.
    let edit_enabled = state.read().expect("state lock").edit_enabled;
    match build_site(docs, LinkPolicy::Lenient, &excluder, edit_enabled) {
        Ok(mut site) => {
            let next_epoch = state.read().expect("state lock").epoch + 1;
            match build_pages(
                cfg,
                &mut site,
                project_dir,
                next_epoch,
                &excluder,
                edit_enabled,
            ) {
                Ok((pages, root_assets, editable)) => {
                    let mut s = state.write().expect("state lock");
                    s.pages = pages;
                    s.epoch = next_epoch;
                    s.excluder = excluder;
                    s.root_assets = root_assets;
                    s.editable = editable;
                }
                Err(e) => eprintln!("rebuild failed, keeping last good site: {e:#}"),
            }
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

pub(crate) fn serve_loop(server: &Server, state: Arc<RwLock<ServedSite>>, docs: PathBuf) {
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

/// Whether a bind host is loopback — the gate for edit capability. Parse as an
/// IP and ask the stdlib; treat the literal "localhost" as loopback; everything
/// else (including names we can't resolve here and unspecified `0.0.0.0`/`::`)
/// is treated as non-loopback so editing is never enabled by accident.
fn host_is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
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
    /// `None` = live-reload disabled. The caller must keep this alive; see `spawn_watcher`.
    watcher: Option<(RecommendedWatcher, JoinHandle<()>)>,
}

/// Load the config, build the site, start the watcher, and bind the port — everything up to
/// (but not including) the blocking request loop.
fn setup(project_dir: &Path, host: &str, port: Option<u16>) -> Result<Serving> {
    let cfg = SiteConfig::load(project_dir)?;
    let docs = cfg.docs_path(project_dir);

    let excluder = Arc::new(Excluder::new(project_dir, &docs, &cfg.exclude));
    for w in excluder.warnings() {
        eprintln!("warning: {w}");
    }

    // The readonly opt-out (Task 9, for embedded hosts) is not wired yet —
    // `run_serve` and `serve_handle` both get the loopback-derived flag as-is.
    let edit_enabled = host_is_loopback(host);

    let mut site = build_site(&docs, LinkPolicy::Lenient, &excluder, edit_enabled)?;
    let (pages, root_assets, editable) =
        build_pages(&cfg, &mut site, project_dir, 0, &excluder, edit_enabled)?;
    let state = Arc::new(RwLock::new(ServedSite {
        pages,
        epoch: 0,
        excluder,
        root_assets,
        editable,
        edit_enabled,
    }));

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
        watcher,
    })
}

pub fn run_serve(project_dir: &Path, host: &str, port: Option<u16>, open: bool) -> Result<()> {
    let Serving {
        server,
        state,
        docs,
        // Held until this function returns; dropping it would disable live-reload.
        watcher: _watcher,
    } = setup(project_dir, host, port)?;

    let listen = server.server_addr();
    println!("compositor serving {} on http://{listen}/", docs.display());
    if open {
        open_browser(&format!("http://{listen}/"));
    }
    serve_loop(&server, state, docs);
    Ok(())
}

/// A running site: bound, serving, and watching — shut down on demand.
///
/// Returned by [`serve_handle`] once the port is bound, so a host app can start a site and
/// immediately point a webview at `port`. Shutdown is idempotent and also runs on drop, so a
/// dropped handle never leaks its threads.
///
/// A returned handle means *bound*, not *fully healthy*: live-reload can be dead on arrival
/// (the watcher failed to start) while the site still serves. Check [`ServeHandle::live_reload`]
/// rather than treating a successful return as a live site.
pub struct ServeHandle {
    /// The bound loopback port. Assigned by the OS (`:0`), so it is never "address in use".
    pub port: u16,
    server: Option<Arc<Server>>,
    serve_thread: Option<JoinHandle<()>>,
    watcher: Option<(RecommendedWatcher, JoinHandle<()>)>,
}

impl ServeHandle {
    /// Whether live-reload is running for this site.
    ///
    /// `false` means the watcher failed to start (see `spawn_watcher`) and the site is serving
    /// a frozen render: pages still resolve, but no edit will ever reach them. Nothing else
    /// surfaces that to an embedded host — the reason goes to stderr, which is the CLI's channel,
    /// not a host app's — so a "bound port" does not mean "live site". A host driving a liveness
    /// indicator should read this rather than infer liveness from `serve_handle` having returned.
    ///
    /// Derived from the watcher this handle owns rather than stored, so it cannot drift from the
    /// truth. It reports `false` once the handle is stopped, which is honest: a stopped site is
    /// not live-reloading.
    pub fn live_reload(&self) -> bool {
        self.watcher.is_some()
    }

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
        watcher,
    } = setup(project_dir, "127.0.0.1", None)?;

    let port = server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .ok_or_else(|| anyhow!("serve bound a non-IP address"))?;

    let server = Arc::new(server);
    let loop_server = Arc::clone(&server);
    let serve_thread = std::thread::spawn(move || serve_loop(&loop_server, state, docs));

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

    fn sample_state(excluder: Excluder) -> Arc<RwLock<ServedSite>> {
        let mut pages = HashMap::new();
        pages.insert(
            "index.html".to_string(),
            inject_reload("<body>Hello</body>", 0),
        );
        Arc::new(RwLock::new(ServedSite {
            pages,
            epoch: 0,
            excluder: Arc::new(excluder),
            root_assets: HashMap::new(),
            editable: HashMap::new(),
            edit_enabled: false,
        }))
    }

    fn get(addr: std::net::SocketAddr, path: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        resp
    }

    fn post(addr: std::net::SocketAddr, path: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(req.as_bytes()).unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        resp
    }

    #[test]
    fn serves_page_and_reload_endpoint() {
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        // A scoped scratch dir, not the real `/tmp`: `Excluder::new` walks
        // `docs_dir` looking for `.gitignore` files, so pointing it at `/tmp`
        // itself walks every file under it (and would walk a whole git repo
        // if any `/tmp` ancestor ever became one).
        let docs =
            std::env::temp_dir().join(format!("compositor-serve-basic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&docs);
        std::fs::create_dir_all(&docs).unwrap();
        let state = sample_state(Excluder::new(&docs, &docs, &[]));
        let docs_for_thread = docs.clone();
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs_for_thread));

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

        std::fs::remove_dir_all(&docs).ok();
    }

    #[test]
    fn serves_embedded_shell_css() {
        let docs = std::path::PathBuf::from(".");
        let state = sample_state(Excluder::new(&docs, &docs, &[]));
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs));
        let css = get(addr, "/assets/compositor.css");
        assert!(css.contains(".topbar"));
    }

    #[test]
    fn serves_embedded_shell_js() {
        let docs = std::path::PathBuf::from(".");
        let state = sample_state(Excluder::new(&docs, &docs, &[]));
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs));
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

        let state = sample_state(Excluder::new(&tmp, &tmp, &["superpowers/".to_string()]));
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let docs_for_thread = tmp.clone();
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs_for_thread));

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
    fn on_demand_asset_honors_gitignore() {
        // Same exposure as `on_demand_asset_honors_exclude`, via the other rule:
        // a gitignored tree must not be servable by direct URL either, or
        // `serve --host 0.0.0.0` hands out the scratch the rule exists to hide.
        let tmp = std::env::temp_dir().join(format!("compositor-serve-gi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".git")).unwrap();
        std::fs::create_dir_all(tmp.join("superpowers")).unwrap();
        std::fs::create_dir_all(tmp.join("guides")).unwrap();
        std::fs::write(tmp.join(".gitignore"), "superpowers/\n").unwrap();
        std::fs::write(tmp.join("superpowers/note.txt"), "secret").unwrap();
        std::fs::write(tmp.join("guides/kept.txt"), "public").unwrap();

        // No `exclude` patterns at all: gitignore alone must hide the tree.
        let state = sample_state(Excluder::new(&tmp, &tmp, &[]));
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let docs_for_thread = tmp.clone();
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs_for_thread));

        let excluded = get(addr, "/superpowers/note.txt");
        assert!(
            excluded.contains("404"),
            "gitignored asset must not be served: {excluded}"
        );

        let kept = get(addr, "/guides/kept.txt");
        assert!(kept.contains("200 OK"), "kept asset resp: {kept}");
        assert!(kept.contains("public"), "kept asset resp: {kept}");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn on_demand_docs_asset_wins_a_root_asset_url_collision() {
        // Same global constraint as `build.rs`'s
        // `docs_asset_wins_a_url_collision_with_a_repo_root_asset`, over HTTP:
        // when a docs-dir file and a `root_assets` entry (a repo-root README/CLAUDE
        // image) claim the same site url, docs wins. The whole mechanism here is
        // `handle`'s on-demand docs branch running — and returning — before the
        // `root_assets` branch; reorder those two and this test must catch it.
        let tmp =
            std::env::temp_dir().join(format!("compositor-serve-collision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("images")).unwrap();
        std::fs::write(tmp.join("images/logo.png"), "DOCS").unwrap();

        let outside = std::env::temp_dir().join(format!(
            "compositor-serve-collision-outside-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("logo.png"), "OUTSIDE").unwrap();

        let mut root_assets = HashMap::new();
        root_assets.insert("images/logo.png".to_string(), outside.join("logo.png"));
        let state = Arc::new(RwLock::new(ServedSite {
            pages: HashMap::new(),
            epoch: 0,
            excluder: Arc::new(Excluder::new(&tmp, &tmp, &[])),
            root_assets,
            editable: HashMap::new(),
            edit_enabled: false,
        }));

        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let docs_for_thread = tmp.clone();
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, docs_for_thread));

        let resp = get(addr, "/images/logo.png");
        assert!(resp.contains("200 OK"), "resp: {resp}");
        assert!(
            resp.contains("DOCS"),
            "on-disk docs content must win: {resp}"
        );
        assert!(!resp.contains("OUTSIDE"), "resp: {resp}");

        std::fs::remove_dir_all(&tmp).ok();
        std::fs::remove_dir_all(&outside).ok();
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
        let excluder = Arc::new(Excluder::new(&tmp, &docs, &[]));
        let mut site = build_site(&docs, LinkPolicy::Lenient, &excluder, false).unwrap();
        let (pages, root_assets, editable) =
            build_pages(&cfg, &mut site, &tmp, 0, &excluder, false).unwrap();
        let state = RwLock::new(ServedSite {
            pages,
            epoch: 0,
            excluder,
            root_assets,
            editable,
            edit_enabled: false,
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
        let state = sample_state(Excluder::new(&tmp, &docs, &[]));
        let (watcher, thread) =
            spawn_watcher(state, cfg, docs, tmp.clone()).expect("watcher starts");

        drop(watcher);
        thread
            .join()
            .expect("rebuild thread must exit once its watcher is dropped");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn spawn_watcher_returns_none_when_the_path_cannot_be_watched() {
        // The `None` branch is what `ServeHandle::live_reload()` reports as `false`, so it has to
        // be reachable for that report to mean anything. Watching a path that does not exist is
        // the forceable version of "the watcher failed to start".
        let missing =
            std::env::temp_dir().join(format!("compositor-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);

        let cfg = SiteConfig {
            site_name: "T".to_string(),
            ..Default::default()
        };
        let state = sample_state(Excluder::new(&missing, &missing, &[]));
        assert!(
            spawn_watcher(state, cfg, missing.clone(), missing).is_none(),
            "watching a non-existent dir must disable live-reload, not succeed"
        );
    }

    #[test]
    fn serve_handle_reports_live_reload_state() {
        // A bound port must not be mistaken for a live site: the host has no other signal that
        // live-reload died (the failure only ever went to stderr, which an embedded host never
        // sees), so the handle has to say so itself.
        let tmp = std::env::temp_dir().join(format!("compositor-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        std::fs::write(tmp.join("docs/index.md"), "# Live\n").unwrap();

        let h = serve_handle(&tmp).expect("serve_handle binds");
        assert!(
            h.live_reload(),
            "a watchable docs tree must report live-reload running"
        );
        h.shutdown();

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

    #[test]
    fn loopback_hosts_enable_editing_others_do_not() {
        assert!(host_is_loopback("127.0.0.1"));
        assert!(host_is_loopback("localhost"));
        assert!(host_is_loopback("::1"));
        assert!(host_is_loopback("127.5.5.5")); // whole 127/8 is loopback
        assert!(!host_is_loopback("0.0.0.0"));
        assert!(!host_is_loopback("::"));
        assert!(!host_is_loopback("192.168.1.10"));
        assert!(!host_is_loopback("example.com")); // unknown name -> not editable
    }

    #[test]
    fn edit_pages_carry_toggle_and_payload_and_map_source() {
        let tmp = std::env::temp_dir().join(format!("comp-editpages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("index.md"), "# One\n\npara\n").unwrap();
        let cfg = SiteConfig {
            site_name: "T".into(),
            docs_dir: Some("docs".into()),
            ..Default::default()
        };
        let ex = Arc::new(Excluder::new(&tmp, &docs, &[]));

        let mut site = build_site(&docs, LinkPolicy::Lenient, &ex, true).unwrap();
        let (pages, _root, editable) = build_pages(&cfg, &mut site, &tmp, 0, &ex, true).unwrap();

        let idx = &pages["index.html"];
        assert!(
            idx.contains(r#"class="edit-toggle"#),
            "toggle injected: {idx}"
        );
        assert!(idx.contains(r#"id="__editsrc""#), "payload injected");
        assert!(idx.contains("editor.js"), "editor script injected");
        assert_eq!(
            editable["index.html"],
            docs.join("index.md"),
            "url maps to source file"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn build_pages_without_edit_are_clean() {
        let tmp = std::env::temp_dir().join(format!("comp-noedit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("index.md"), "# One\n").unwrap();
        let cfg = SiteConfig {
            site_name: "T".into(),
            docs_dir: Some("docs".into()),
            ..Default::default()
        };
        let ex = Arc::new(Excluder::new(&tmp, &docs, &[]));
        let mut site = build_site(&docs, LinkPolicy::Lenient, &ex, false).unwrap();
        let (pages, _root, editable) = build_pages(&cfg, &mut site, &tmp, 0, &ex, false).unwrap();
        assert!(!pages["index.html"].contains("edit-toggle"));
        assert!(!pages["index.html"].contains("editor.js"));
        assert!(editable.is_empty());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn edit_endpoint_writes_only_mapped_urls_on_loopback() {
        let tmp = std::env::temp_dir().join(format!("comp-editep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let src = docs.join("index.md");
        std::fs::write(&src, "# One\n\npara\n").unwrap();

        let mut editable = HashMap::new();
        editable.insert("index.html".to_string(), src.clone());
        let state = Arc::new(RwLock::new(ServedSite {
            pages: HashMap::new(),
            epoch: 0,
            excluder: Arc::new(Excluder::new(&tmp, &docs, &[])),
            root_assets: HashMap::new(),
            editable,
            edit_enabled: true,
        }));
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let d = docs.clone();
        let s = std::sync::Arc::clone(&server);
        std::thread::spawn(move || serve_loop(&s, state, d));

        // A mapped url writes the file.
        let ok = post(
            addr,
            "/__edit",
            r##"{"url":"index.html","source":"# Edited\n\npara\n"}"##,
        );
        assert!(ok.contains("200 OK"), "resp: {ok}");
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "# Edited\n\npara\n");

        // An unmapped url is refused — cannot write an arbitrary path.
        let bad = post(addr, "/__edit", r#"{"url":"../etc/x.html","source":"pwn"}"#);
        assert!(bad.contains("403"), "resp: {bad}");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn serve_serves_a_repo_root_readme_asset() {
        let tmp =
            std::env::temp_dir().join(format!("compositor-serve-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        std::fs::create_dir_all(tmp.join("images")).unwrap();
        std::fs::write(tmp.join("images/logo.png"), "PNGDATA").unwrap();
        std::fs::write(tmp.join("README.md"), "# P\n\n![logo](images/logo.png)\n").unwrap();
        std::fs::write(tmp.join("docs/guide.md"), "# Guide\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\n",
        )
        .unwrap();

        let h = serve_handle(&tmp).expect("serve_handle binds");
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], h.port));

        let home = get(addr, "/");
        assert!(home.contains(r#"src="images/logo.png""#), "home: {home}");

        // The asset lives outside the docs dir, so the docs on-demand branch can
        // never find it — it must come from the recorded root-asset map.
        let img = get(addr, "/images/logo.png");
        assert!(img.contains("200 OK"), "img resp: {img}");
        assert!(img.contains("PNGDATA"), "img resp: {img}");

        h.shutdown();
        std::fs::remove_dir_all(&tmp).ok();
    }
}
