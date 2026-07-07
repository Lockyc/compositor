use anyhow::{anyhow, Result};
use comrak::nodes::{AstNode, NodeValue};
use comrak::plugins::syntect::SyntectAdapter;
use comrak::{format_html_with_plugins, parse_document, Arena, Options, Plugins};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct Rendered {
    pub html: String,
    pub first_h1: Option<String>,
}

/// Whether an unresolvable internal link is a hard error (`build`) or is
/// tolerated and left as an honest broken link (`serve`, which must never
/// halt an unattended rebuild).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkPolicy {
    Strict,
    Lenient,
}

pub fn comrak_options<'c>() -> Options<'c> {
    let mut o = Options::default();
    o.extension.table = true;
    o.extension.tasklist = true;
    o.extension.strikethrough = true;
    o.extension.autolink = true;
    o.extension.header_ids = Some(String::new()); // heading anchors
    o
}

pub fn render_markdown(
    body: &str,
    page_dir: &Path,
    known_urls: &HashSet<String>,
    policy: LinkPolicy,
) -> Result<Rendered> {
    let arena = Arena::new();
    let options = comrak_options();
    let root = parse_document(&arena, body, &options);

    let first_h1 = find_first_h1(root);

    for node in root.descendants() {
        let new_url = {
            let data = node.data.borrow();
            if let NodeValue::Link(ref link) = data.value {
                rewrite_link(&link.url, page_dir, known_urls, policy)?
            } else {
                None
            }
        };
        if let Some(new_url) = new_url {
            let mut data = node.data.borrow_mut();
            if let NodeValue::Link(ref mut link) = data.value {
                link.url = new_url;
            }
        }
    }

    let adapter = SyntectAdapter::new(Some("InspiredGitHub"));
    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&adapter);

    let mut out = Vec::new();
    format_html_with_plugins(root, &options, &mut out, &plugins)
        .expect("comrak html formatting is infallible for in-memory writer");
    Ok(Rendered {
        html: String::from_utf8(out).expect("comrak emits valid utf-8"),
        first_h1,
    })
}

fn rewrite_link(
    url: &str,
    page_dir: &Path,
    known: &HashSet<String>,
    policy: LinkPolicy,
) -> Result<Option<String>> {
    // Skip external / anchor / non-.md links.
    if url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("mailto:")
        || url.starts_with('#')
        || !url.contains(".md")
    {
        return Ok(None);
    }
    // Split off any anchor fragment.
    let (path_part, frag) = match url.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (url, None),
    };
    // Resolve relative to the page's directory, normalizing `..`/`.`, purely
    // to validate the link against the known (site-root-relative) urls.
    let joined = page_dir.join(path_part);
    let normalized = normalize(&joined);
    let resolved_target = md_ext_to_html(&normalized.to_string_lossy());
    // The emitted href stays relative to the page (only the extension
    // changes) — we don't want to rewrite "tar.md" into "cli/tar.html".
    let new_path = md_ext_to_html(path_part);
    if !known.contains(&resolved_target) {
        match policy {
            LinkPolicy::Strict => {
                return Err(anyhow!(
                    "unresolvable internal link: {url} (from {})",
                    page_dir.display()
                ));
            }
            // Lenient: fall through and emit the rewritten (dead) href so the
            // rebuild never aborts. The link 404s in the browser — visible, not
            // swallowed.
            LinkPolicy::Lenient => {}
        }
    }
    Ok(Some(match frag {
        Some(f) => format!("{new_path}#{f}"),
        None => new_path,
    }))
}

/// Swap a *trailing* `.md` for `.html`, leaving any earlier `.md` substring
/// intact — mirroring `site::url_for`'s `strip_suffix(".md")`. An
/// all-occurrences replace would corrupt a path like `notes.md.md`.
fn md_ext_to_html(path: &str) -> String {
    match path.strip_suffix(".md") {
        Some(stem) => format!("{stem}.html"),
        None => path.to_string(),
    }
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        use std::path::Component::*;
        match c {
            ParentDir => {
                out.pop();
            }
            CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn find_first_h1<'a>(node: &'a AstNode<'a>) -> Option<String> {
    for child in node.descendants() {
        if let NodeValue::Heading(h) = &child.data.borrow().value {
            if h.level == 1 {
                return Some(text_of(child));
            }
        }
    }
    None
}

fn text_of<'a>(node: &'a AstNode<'a>) -> String {
    let mut s = String::new();
    for d in node.descendants() {
        match &d.data.borrow().value {
            NodeValue::Text(t) => s.push_str(t),
            NodeValue::Code(code) => s.push_str(&code.literal),
            _ => {}
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_gfm_table() {
        let r = render_markdown(
            "| a | b |\n|---|---|\n| 1 | 2 |",
            Path::new(""),
            &HashSet::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert!(r.html.contains("<table>"));
    }

    #[test]
    fn highlights_fenced_code() {
        let r = render_markdown(
            "```rust\nfn main() {}\n```",
            Path::new(""),
            &HashSet::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        // syntect emits inline styles on a <pre>/<code> span structure
        assert!(r.html.contains("style=\"") && r.html.contains("main"));
    }

    #[test]
    fn extracts_first_h1() {
        let r = render_markdown(
            "# Title Here\n\nbody",
            Path::new(""),
            &HashSet::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert_eq!(r.first_h1.as_deref(), Some("Title Here"));
    }

    #[test]
    fn no_h1_is_none() {
        let r = render_markdown(
            "## Sub only\n\nbody",
            Path::new(""),
            &HashSet::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert!(r.first_h1.is_none());
    }

    #[test]
    fn extracts_first_h1_with_inline_code() {
        let r = render_markdown(
            "# Prettier `just` recipe list (`x.sh`)",
            Path::new(""),
            &HashSet::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert_eq!(
            r.first_h1,
            Some("Prettier just recipe list (x.sh)".to_string())
        );
    }
}
