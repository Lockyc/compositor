use crate::wikilink::{relative_url, WikiIndex, WikiResolution, WikiTarget};
use anyhow::{anyhow, Result};
use comrak::nodes::{AstNode, NodeLink, NodeValue, NodeWikiLink};
use comrak::plugins::syntect::SyntectAdapter;
use comrak::{format_html_with_plugins, parse_document, Anchorizer, Arena, Options, Plugins};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Rendered {
    pub html: String,
    pub first_h1: Option<String>,
    pub toc: Vec<TocEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TocEntry {
    pub level: u8,
    pub id: String,
    pub text: String,
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
    // heading anchors. The empty prefix is load-bearing: `collect_toc` assumes
    // comrak's emitted heading ids are the bare Anchorizer slug with no
    // prefix — changing this string here would desync TOC hrefs from anchors.
    o.extension.header_ids = Some(String::new());
    o.extension.wikilinks_title_after_pipe = true;
    o
}

/// Parse just enough to return the first H1's text — used by `build_site` pass 1
/// to resolve a page's title before the full render pass.
pub fn first_h1(body: &str) -> Option<String> {
    let arena = Arena::new();
    let options = comrak_options();
    let root = parse_document(&arena, body, &options);
    find_first_h1(root)
}

pub fn render_markdown(
    body: &str,
    page_dir: &Path,
    known_urls: &HashSet<String>,
    wiki: &WikiIndex,
    policy: LinkPolicy,
) -> Result<Rendered> {
    let arena = Arena::new();
    let options = comrak_options();
    let root = parse_document(&arena, body, &options);

    let first_h1 = find_first_h1(root);
    let toc = collect_toc(root);

    // First pass (read-only w.r.t. tree shape): rewrite md-link urls in place and
    // plan each wikilink's replacement. We defer tree surgery (detach/append) to a
    // second pass so we never mutate structure while iterating descendants.
    let mut wl_actions: Vec<(&AstNode, WikiAction)> = Vec::new();
    for node in root.descendants() {
        enum Kind {
            Link(Option<String>),
            Wiki(WikiAction),
            None,
        }
        let kind = {
            let data = node.data.borrow();
            match &data.value {
                NodeValue::Link(link) => {
                    Kind::Link(rewrite_link(&link.url, page_dir, known_urls, policy)?)
                }
                NodeValue::WikiLink(wl) => Kind::Wiki(plan_wikilink(
                    &wl.url,
                    &text_of(node),
                    page_dir,
                    wiki,
                    policy,
                )?),
                _ => Kind::None,
            }
        };
        match kind {
            Kind::Link(Some(new_url)) => {
                let mut data = node.data.borrow_mut();
                if let NodeValue::Link(ref mut link) = data.value {
                    link.url = new_url;
                }
            }
            Kind::Wiki(action) => wl_actions.push((node, action)),
            _ => {}
        }
    }
    // Second pass: apply wikilink tree surgery using the arena for new text nodes.
    for (node, action) in wl_actions {
        apply_wikilink(node, action, &arena);
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
        toc,
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

/// The planned replacement for one `[[wikilink]]` node.
enum WikiAction {
    /// Resolved (or the lenient pick of an ambiguous set): become a plain `<a>`.
    Link { href: String, text: String },
    /// Unresolved under the lenient policy: stay a `WikiLink` with an empty url so
    /// the formatter emits `<a href="" data-wikilink="true">…</a>` (the CSS hook).
    Dead { text: String },
}

/// Decide what a wikilink becomes, or error under the strict policy.
fn plan_wikilink(
    raw_url: &str,
    label: &str,
    page_dir: &Path,
    wiki: &WikiIndex,
    policy: LinkPolicy,
) -> Result<WikiAction> {
    // comrak's `clean_url` (applied to a parsed wikilink's url) trims, unescapes
    // HTML entities, and un-backslash-escapes — it does NOT percent-encode. So
    // `raw_url` is already the plain typed name (spaces intact); no decode step
    // is needed, and running one would corrupt a name containing a literal
    // `%xx` sequence.
    let (name, frag) = match raw_url.split_once('#') {
        Some((n, f)) => (n, Some(f)),
        None => (raw_url, None),
    };
    // Bare `[[name]]` when comrak's default label equals the typed url; a piped
    // `[[name|label]]` differs, so the author's label is used verbatim.
    let bare = label == raw_url;

    let make = |t: &WikiTarget| -> WikiAction {
        let mut href = relative_url(page_dir, &t.url);
        if let Some(f) = frag {
            href.push('#');
            href.push_str(f);
        }
        let text = if bare {
            t.title.clone()
        } else {
            label.to_string()
        };
        WikiAction::Link { href, text }
    };

    match wiki.resolve(name) {
        WikiResolution::Resolved(t) => Ok(make(&t)),
        WikiResolution::Ambiguous(cands) => match policy {
            LinkPolicy::Strict => Err(anyhow!(
                "ambiguous wikilink: [[{name}]] matches {} (from {})",
                cands
                    .iter()
                    .map(|c| c.url.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                page_dir.display()
            )),
            LinkPolicy::Lenient => Ok(make(&cands[0])),
        },
        WikiResolution::Unresolved => match policy {
            LinkPolicy::Strict => Err(anyhow!(
                "unresolvable wikilink: [[{name}]] (from {})",
                page_dir.display()
            )),
            LinkPolicy::Lenient => Ok(WikiAction::Dead {
                text: if bare {
                    name.to_string()
                } else {
                    label.to_string()
                },
            }),
        },
    }
}

/// Apply a planned wikilink action: replace the node's children with a single
/// text node and swap its value to a plain `Link` (resolved) or leave it a
/// `WikiLink` with an empty url (dead).
fn apply_wikilink<'a>(node: &'a AstNode<'a>, action: WikiAction, arena: &'a Arena<AstNode<'a>>) {
    for child in node.children() {
        child.detach();
    }
    let (value, text) = match action {
        WikiAction::Link { href, text } => (
            NodeValue::Link(NodeLink {
                url: href,
                title: String::new(),
            }),
            text,
        ),
        WikiAction::Dead { text } => (
            NodeValue::WikiLink(NodeWikiLink { url: String::new() }),
            text,
        ),
    };
    node.data.borrow_mut().value = value;
    node.append(arena.alloc(NodeValue::Text(text).into()));
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

/// Collect the page's `h2`/`h3` headings for the table of contents. Every
/// heading (all levels) is fed through a single `Anchorizer` in document order
/// so the generated ids match comrak's own `header_ids` output exactly —
/// comrak dedupes repeated slugs with numeric suffixes across all headings, so
/// skipping any (even h1/h4) would desync the numbering.
fn collect_toc<'a>(root: &'a AstNode<'a>) -> Vec<TocEntry> {
    let mut anchorizer = Anchorizer::new();
    let mut toc = Vec::new();
    for node in root.descendants() {
        let level = match &node.data.borrow().value {
            NodeValue::Heading(h) => h.level,
            _ => continue,
        };
        let text = text_of(node);
        let id = anchorizer.anchorize(text.clone());
        if level == 2 || level == 3 {
            toc.push(TocEntry { level, id, text });
        }
    }
    toc
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
    use crate::wikilink::WikiIndex;
    use std::path::PathBuf;

    fn wiki_fixture() -> WikiIndex {
        let mut w = WikiIndex::new();
        w.add_page(
            "guide/getting-started.html",
            "Getting Started",
            &PathBuf::from("guide/getting-started.md"),
            "getting-started",
            &[],
        );
        w
    }

    #[test]
    fn renders_gfm_table() {
        let r = render_markdown(
            "| a | b |\n|---|---|\n| 1 | 2 |",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
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
            &WikiIndex::new(),
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
            &WikiIndex::new(),
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
            &WikiIndex::new(),
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
            &WikiIndex::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert_eq!(
            r.first_h1,
            Some("Prettier just recipe list (x.sh)".to_string())
        );
    }

    #[test]
    fn toc_collects_h2_and_h3_only() {
        let r = render_markdown(
            "# Title\n\n## Alpha\n\ntext\n\n### Beta\n\n#### Deep\n\ntext",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        let levels: Vec<u8> = r.toc.iter().map(|t| t.level).collect();
        let texts: Vec<&str> = r.toc.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(levels, vec![2, 3]); // h1 and h4 excluded
        assert_eq!(texts, vec!["Alpha", "Beta"]);
    }

    #[test]
    fn toc_ids_match_emitted_heading_anchors() {
        let r = render_markdown(
            "## Getting Started\n\ntext",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        let id = &r.toc[0].id;
        assert_eq!(id, "getting-started");
        // The id must resolve to an anchor comrak actually emitted in the HTML.
        assert!(
            r.html.contains(&format!("id=\"{id}\"")),
            "emitted html missing id={id}: {}",
            r.html
        );
    }

    #[test]
    fn toc_dedup_numbering_tracks_comrak() {
        // Two identical headings: comrak suffixes the second id (`-1`). The TOC
        // must use the same suffixing, which requires anchorizing every heading in
        // document order through one shared Anchorizer.
        let r = render_markdown(
            "## Dup\n\n## Dup\n\ntext",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        let ids: Vec<&str> = r.toc.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["dup", "dup-1"]);
        assert!(r.html.contains("id=\"dup\"") && r.html.contains("id=\"dup-1\""));
    }

    #[test]
    fn toc_empty_when_no_subheadings() {
        let r = render_markdown(
            "# Only H1\n\nbody with no sub-headings",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert!(r.toc.is_empty());
    }

    #[test]
    fn bare_wikilink_renders_target_title_as_link_text() {
        // Typed as a stem; display text must be the resolved page's title.
        let r = render_markdown(
            "See [[getting-started]].",
            Path::new(""),
            &HashSet::new(),
            &wiki_fixture(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert!(
            r.html
                .contains(r#"<a href="guide/getting-started.html">Getting Started</a>"#),
            "got: {}",
            r.html
        );
        // Resolved wikilinks are plain <a> — no data-wikilink attribute.
        assert!(!r.html.contains("data-wikilink"));
    }

    #[test]
    fn piped_wikilink_uses_author_label() {
        let r = render_markdown(
            "[[getting-started|start here]]",
            Path::new(""),
            &HashSet::new(),
            &wiki_fixture(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert!(
            r.html
                .contains(r#"<a href="guide/getting-started.html">start here</a>"#),
            "got: {}",
            r.html
        );
    }

    #[test]
    fn wikilink_anchor_is_appended_to_href() {
        let r = render_markdown(
            "[[Getting Started#install]]",
            Path::new(""),
            &HashSet::new(),
            &wiki_fixture(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert!(
            r.html
                .contains(r##"href="guide/getting-started.html#install""##),
            "got: {}",
            r.html
        );
    }

    #[test]
    fn wikilink_href_is_page_relative() {
        // A page in admin/ links to a page in guide/ -> ../ prefix.
        let r = render_markdown(
            "[[Getting Started]]",
            Path::new("admin"),
            &HashSet::new(),
            &wiki_fixture(),
            LinkPolicy::Strict,
        )
        .unwrap();
        assert!(
            r.html.contains(r#"href="../guide/getting-started.html""#),
            "got: {}",
            r.html
        );
    }

    #[test]
    fn unresolved_wikilink_is_error_under_strict() {
        let err = render_markdown(
            "[[No Such Page]]",
            Path::new(""),
            &HashSet::new(),
            &wiki_fixture(),
            LinkPolicy::Strict,
        )
        .unwrap_err();
        assert!(err.to_string().contains("No Such Page"), "got: {err}");
    }

    #[test]
    fn unresolved_wikilink_is_dead_anchor_under_lenient() {
        let r = render_markdown(
            "[[No Such Page]]",
            Path::new(""),
            &HashSet::new(),
            &wiki_fixture(),
            LinkPolicy::Lenient,
        )
        .unwrap();
        // Neutered anchor keeps the data-wikilink attribute and has no valid href.
        assert!(
            r.html.contains(r#"data-wikilink="true""#),
            "got: {}",
            r.html
        );
        assert!(r.html.contains("No Such Page"), "got: {}", r.html);
        assert!(r.html.contains(r#"href="""#), "got: {}", r.html);
    }

    #[test]
    fn ambiguous_wikilink_errors_under_strict_picks_first_under_lenient() {
        let mut w = WikiIndex::new();
        w.add_page(
            "admin/setup.html",
            "Admin",
            &PathBuf::from("admin/setup.md"),
            "setup",
            &[],
        );
        w.add_page(
            "ref/setup.html",
            "Ref",
            &PathBuf::from("ref/setup.md"),
            "setup",
            &[],
        );

        let err = render_markdown(
            "[[setup]]",
            Path::new(""),
            &HashSet::new(),
            &w,
            LinkPolicy::Strict,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("admin/setup.html")
                && err.to_string().contains("ref/setup.html"),
            "got: {err}"
        );

        let r = render_markdown(
            "[[setup]]",
            Path::new(""),
            &HashSet::new(),
            &w,
            LinkPolicy::Lenient,
        )
        .unwrap();
        assert!(
            r.html.contains(r#"href="admin/setup.html""#),
            "got: {}",
            r.html
        ); // sorted-first
    }

    #[test]
    fn wikilink_inside_code_span_is_untouched() {
        let r = render_markdown(
            "`[[getting-started]]`",
            Path::new(""),
            &HashSet::new(),
            &wiki_fixture(),
            LinkPolicy::Strict,
        )
        .unwrap();
        // Comrak does not parse wikilinks inside code spans, so it stays literal text.
        assert!(r.html.contains("[[getting-started]]"), "got: {}", r.html);
        assert!(!r.html.contains("<a href"), "got: {}", r.html);
    }
}
