//! `build` and `serve` must agree on what a tree publishes: every file the
//! build writes is a 200 under serve, and every source path the build leaves
//! out is a 404.

use compositor::build::run_build;
use compositor::serve::serve_handle_with;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use render_core::LinkPolicy;
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// What a browser percent-encodes in a path segment (plus `%` itself, which a
/// literal file name may carry).
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'%');

fn browser_path(url: &str) -> String {
    let encoded: Vec<String> = url
        .split('/')
        .map(|seg| utf8_percent_encode(seg, SEGMENT).to_string())
        .collect();
    format!("/{}", encoded.join("/"))
}

fn status(port: u16, url: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n",
        browser_path(url)
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).unwrap();
    let head = String::from_utf8_lossy(&resp[..resp.len().min(16)]).into_owned();
    head.split(' ').nth(1).unwrap().parse().unwrap()
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// Every file (or symlink) under `root`, as a `/`-joined relative path.
fn files_under(root: &Path) -> BTreeSet<String> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| !e.file_type().is_dir())
        .map(|e| {
            e.path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

/// The url a source path would publish at: a page as `.html`, anything else
/// as itself.
fn source_url(rel: &str) -> String {
    match rel.strip_suffix(".md") {
        Some(stem) => format!("{stem}.html"),
        None => rel.to_string(),
    }
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("compositor-parity-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    // Canonical, so the fixture's absolute symlink targets compare the way
    // the `Excluder`'s containment check sees them (macOS `/tmp`).
    fs::canonicalize(&d).unwrap()
}

#[cfg(unix)]
#[test]
fn build_output_and_serve_agree_on_a_hostile_tree() {
    use std::os::unix::fs::symlink;

    let base = scratch("hostile");
    let proj = base.join("proj");
    let outside = base.join("outside");
    write(&outside, "secret.png", "outside the repo");

    // A bare folder that is its own repo root and docs dir, so `.git`, the
    // out dir and its staging debris all sit inside the tree being published.
    write(
        &proj,
        "compositor.toml",
        "site_name = \"P\"\ndocs_dir = \".\"\nexclude = [\"private/\"]\n",
    );
    write(&proj, ".git/config", "[core]\n");
    write(&proj, ".git/HEAD", "ref: refs/heads/main\n");
    write(&proj, "nested/.git", "gitdir: ../.git/worktrees/nested\n");
    write(&proj, "nested/page.md", "# Nested\n");
    write(&proj, ".gitignore", "scratch/\n");
    write(&proj, "scratch/notes.md", "# Scratch\n");
    write(&proj, "scratch/draft.png", "draft");
    write(&proj, "private/plan.md", "# Plan\n");
    write(&proj, "private/plan.png", "plan");
    write(&proj, "site.tmp-123/stale.png", "debris");
    write(&proj, "img/real.png", "real");
    symlink(proj.join("img/real.png"), proj.join("img/linked.png")).unwrap();
    symlink(outside.join("secret.png"), proj.join("img/escape.png")).unwrap();
    write(
        &proj,
        "index.md",
        "# Home\n\n[notes](Meeting%20Notes.md) [café](café.md)\n\n![r](img/real.png) ![l](img/linked.png)\n",
    );
    write(&proj, "Meeting Notes.md", "# Meeting Notes\n");
    write(&proj, "café.md", "# Café\n");
    write(&proj, "a#b.md", "# Hash\n");
    write(&proj, "100% done.png", "pct");
    write(&proj, "what?.txt", "q");

    // Collected before the build writes `site/`.
    let sources = files_under(&proj);

    run_build(&proj, LinkPolicy::Lenient).unwrap();
    let built = files_under(&proj.join("site"));
    for must in ["index.html", "Meeting Notes.html", "café.html", "a#b.html"] {
        assert!(built.contains(must), "build wrote {must}: {built:?}");
    }

    let handle = serve_handle_with(&proj, false).unwrap();
    let port = handle.port;

    let mut mismatches = Vec::new();
    for url in &built {
        let got = status(port, url);
        if got != 200 {
            mismatches.push(format!("{url}: built, but serve answered {got}"));
        }
    }
    for rel in &sources {
        let url = source_url(rel);
        let expected = if built.contains(&url) { 200 } else { 404 };
        let got = status(port, &url);
        if got != expected {
            mismatches.push(format!(
                "{url} (from {rel}): build {}, serve answered {got}",
                if expected == 200 {
                    "published it"
                } else {
                    "left it out"
                }
            ));
        }
    }
    for forbidden in [
        ".git/config",
        "nested/.git",
        "img/escape.png",
        "private/plan.png",
        "private/plan.html",
        "scratch/draft.png",
        "scratch/notes.html",
        "site.tmp-123/stale.png",
        "site/index.html",
        "site/img/real.png",
    ] {
        assert!(!built.contains(forbidden), "build published {forbidden}");
        let got = status(port, forbidden);
        if got != 404 {
            mismatches.push(format!("{forbidden}: forbidden, serve answered {got}"));
        }
    }
    handle.shutdown();
    assert!(
        mismatches.is_empty(),
        "build and serve disagree:\n{}",
        mismatches.join("\n")
    );
    fs::remove_dir_all(&base).ok();
}

/// What a browser encodes itself when it requests an href: everything but `%`,
/// which it sends as written.
const BROWSER: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// The status serve answers for an `href` as a browser follows it: the path
/// part before any `#fragment` or `?query`, with spaces and non-ASCII encoded.
fn follow(port: u16, href: &str) -> (String, u16) {
    let path = href.split(['#', '?']).next().unwrap_or("");
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .into_owned();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "GET /{} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n",
        utf8_percent_encode(path, BROWSER)
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).unwrap();
    let head = String::from_utf8_lossy(&resp[..resp.len().min(16)]).into_owned();
    (decoded, head.split(' ').nth(1).unwrap().parse().unwrap())
}

fn html_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The href of the link whose text is `title` (a nav entry or a wikilink).
fn nav_href(html: &str, title: &str) -> Option<String> {
    let close = format!("\">{title}</a>");
    let end = html.find(&close)?;
    let start = html[..end].rfind("href=\"")? + "href=\"".len();
    Some(html_unescape(&html[start..end]))
}

/// The href of the pager link with class `class` (`page-nav-prev`/`-next`).
fn pager_href(html: &str, class: &str) -> Option<String> {
    let open = format!("{class}\" href=\"");
    let start = html.find(&open)? + open.len();
    let end = start + html[start..].find('"')?;
    Some(html_unescape(&html[start..end]))
}

mod names {
    use super::*;
    use proptest::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        /// For page names carrying spaces, non-ASCII, `%`, `#` and `?`, the nav,
        /// pager and wikilink hrefs, the file `build` writes and serve's 200 all
        /// name the same page.
        #[test]
        fn rendered_href_build_file_and_serve_line_up(
            raw in prop::collection::vec("[a-zA-Z0-9 é中%#?_-]{1,8}", 1..5)
        ) {
            static N: AtomicUsize = AtomicUsize::new(0);
            let proj = scratch(&format!("names-{}", N.fetch_add(1, Ordering::Relaxed)));
            // Prefixed so no name is promoted to the home page, and deduped
            // case-insensitively for a case-insensitive filesystem.
            let mut seen = BTreeSet::new();
            let names: Vec<String> = raw
                .iter()
                .map(|r| format!("p-{r}"))
                .filter(|n| seen.insert(n.to_lowercase()))
                .collect();
            for (i, n) in names.iter().enumerate() {
                write(&proj, &format!("{n}.md"), &format!("# Page{i}\n"));
            }
            // A home page wikilinking every page whose name a wikilink can
            // spell (`#` starts its fragment).
            let wikilinks: String = names
                .iter()
                .enumerate()
                .filter(|(_, n)| !n.contains('#'))
                .map(|(i, n)| format!("[[{n}|Wiki{i}]]\n\n"))
                .collect();
            write(&proj, "index.md", &format!("# Home\n\n{wikilinks}"));
            run_build(&proj, LinkPolicy::Lenient).unwrap();
            let index = fs::read_to_string(proj.join("site/index.html")).unwrap();
            let handle = serve_handle_with(&proj, false).unwrap();
            for (i, n) in names.iter().enumerate() {
                let file = format!("{n}.html");
                prop_assert!(proj.join("site").join(&file).is_file(), "build wrote {}", file);
                let mut hrefs = Vec::new();
                let href = nav_href(&index, &format!("Page{i}"));
                prop_assert!(href.is_some(), "nav links Page{} ({})", i, n);
                hrefs.push(href.unwrap());
                if !n.contains('#') {
                    let href = nav_href(&index, &format!("Wiki{i}"));
                    prop_assert!(href.is_some(), "home wikilinks Wiki{} ({})", i, n);
                    hrefs.push(href.unwrap());
                }
                for href in hrefs {
                    let (target, got) = follow(handle.port, &href);
                    prop_assert_eq!(&target, &file, "href {:?} names another file", href);
                    prop_assert_eq!(got, 200, "serve answered {} for href {:?}", got, href);
                }
                // Every page's pager links name a built page serve answers.
                let html = fs::read_to_string(proj.join("site").join(&file)).unwrap();
                for class in ["page-nav-prev", "page-nav-next"] {
                    if let Some(href) = pager_href(&html, class) {
                        let (target, got) = follow(handle.port, &href);
                        prop_assert!(
                            proj.join("site").join(&target).is_file(),
                            "{} {} href {:?} names no built file", file, class, href
                        );
                        prop_assert_eq!(got, 200, "serve answered {} for href {:?}", got, href);
                    }
                }
            }
            handle.shutdown();
            fs::remove_dir_all(&proj).ok();
        }
    }
}
