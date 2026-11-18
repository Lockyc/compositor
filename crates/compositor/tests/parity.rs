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
