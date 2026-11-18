//! The editor corpus fixture `editor.test.cjs` reconstructs against: each
//! corpus page as serve renders it in edit mode (the `#doc` article and its
//! `#__editsrc` payload). The committed fixture must match what compositor
//! renders now; `COMPOSITOR_UPDATE_FIXTURES=1 cargo test --test editor_fixture`
//! rewrites it.

use compositor::serve::serve_handle;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;

/// Every construct the editor serializes or must leave alone, each block
/// written in the form the serializer emits, so an unedited block that
/// reconstructs differently has lost or changed authored bytes.
const CORPUS: &str = "---
title: Corpus
aliases:
  - corpus-alias
---

# Corpus

Intro with **bold**, *em*, `code` and ~~struck~~ text.

!!! note \"Heads up\"
    An admonition body.

```rust
fn main() {}
```

- [x] done task
- [ ] open task

A paragraph after a list, past its blank line.

| Left | Centre | Right |
| :-- | :-: | --: |
| a | b | c |

![logo](img/logo.png)

## Custom heading {#custom-id}

> A quote
> over two lines.

1. one
2. two
- plain
- bullets

Last paragraph.
";

fn get(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "GET {path}: {resp}");
    resp.split_once("\r\n\r\n").unwrap().1.to_string()
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> &'a str {
    let a = s.find(start).unwrap_or_else(|| panic!("{start} missing"));
    let b = s[a..].rfind(end).unwrap_or_else(|| panic!("{end} missing")) + a + end.len();
    &s[a..b]
}

/// `{name, doc, payload}` for one served page.
fn fixture_entry(port: u16, name: &str, url: &str) -> serde_json::Value {
    let html = get(port, url);
    let main = between(&html, "<article id=\"doc\"", "</main>");
    let doc = between(main, "<article id=\"doc\"", "</article>");
    let open = "<script type=\"application/json\" id=\"__editsrc\">";
    let payload = &html[html.find(open).expect("payload") + open.len()..];
    let payload = &payload[..payload.find("</script>").unwrap()];
    serde_json::json!({
        "name": name,
        "doc": doc,
        "payload": serde_json::from_str::<serde_json::Value>(payload).unwrap(),
    })
}

#[test]
fn editor_corpus_fixture_is_current() {
    let proj =
        std::env::temp_dir().join(format!("compositor-editor-fixture-{}", std::process::id()));
    let _ = fs::remove_dir_all(&proj);
    fs::create_dir_all(proj.join("docs/img")).unwrap();
    fs::write(proj.join("compositor.toml"), "site_name = \"Fixture\"\n").unwrap();
    fs::write(proj.join("docs/corpus.md"), CORPUS).unwrap();
    fs::write(proj.join("docs/crlf.md"), CORPUS.replace('\n', "\r\n")).unwrap();
    fs::write(proj.join("docs/other.md"), "# Other\n").unwrap();
    fs::write(proj.join("docs/img/logo.png"), b"png").unwrap();

    let handle = serve_handle(&proj).unwrap();
    let fixture = serde_json::json!([
        fixture_entry(handle.port, "lf", "/corpus.html"),
        fixture_entry(handle.port, "crlf", "/crlf.html"),
    ]);
    handle.shutdown();
    fs::remove_dir_all(&proj).ok();

    let rendered = serde_json::to_string_pretty(&fixture).unwrap() + "\n";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/editor-corpus.json");
    if std::env::var_os("COMPOSITOR_UPDATE_FIXTURES").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &rendered).unwrap();
        return;
    }
    let committed = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == rendered,
        "{} is stale: rerun with COMPOSITOR_UPDATE_FIXTURES=1",
        path.display()
    );
}
