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
    // Emit raw HTML (the admonition preprocessor injects <div>/<details>
    // wrappers). Also lets author-written HTML pass through, matching MkDocs/
    // python-markdown. Content is author-trusted, so untrusted-HTML XSS is out
    // of scope.
    o.render.unsafe_ = true;
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
    images: &dyn ImageResolver,
) -> Result<Rendered> {
    let preprocessed = crate::admonitions::preprocess_admonitions(body);
    render_inner(
        &preprocessed,
        page_dir,
        known_urls,
        wiki,
        policy,
        images,
        false,
    )
}

/// Edit-mode sibling of [`render_markdown`], used only by `serve`'s (future)
/// inline-editing path: same rendering, plus comrak `sourcepos` attributes on
/// every emitted block and the admonition preprocessor's per-output-line map
/// back to `body`'s source lines (`None` for a synthesized admonition-wrapper
/// line — see `preprocess_admonitions_mapped`).
pub fn render_markdown_editable(
    body: &str,
    page_dir: &Path,
    known_urls: &HashSet<String>,
    wiki: &WikiIndex,
    policy: LinkPolicy,
    images: &dyn ImageResolver,
) -> Result<(Rendered, Vec<Option<usize>>)> {
    let (preprocessed, line_map) = crate::admonitions::preprocess_admonitions_mapped(body);
    let rendered = render_inner(
        &preprocessed,
        page_dir,
        known_urls,
        wiki,
        policy,
        images,
        true,
    )?;
    Ok((rendered, line_map))
}

/// Shared render body for [`render_markdown`] and [`render_markdown_editable`]:
/// `preprocessed` is the already-admonition-expanded source; `sourcepos`
/// toggles comrak's `data-sourcepos` output (edit mode only — `build` output
/// must stay clean of it).
fn render_inner(
    preprocessed: &str,
    page_dir: &Path,
    known_urls: &HashSet<String>,
    wiki: &WikiIndex,
    policy: LinkPolicy,
    images: &dyn ImageResolver,
    sourcepos: bool,
) -> Result<Rendered> {
    let arena = Arena::new();
    let mut options = comrak_options();
    options.render.sourcepos = sourcepos;
    let root = parse_document(&arena, preprocessed, &options);

    let first_h1 = find_first_h1(root);
    let toc = collect_toc(root);

    // First pass (read-only w.r.t. tree shape): rewrite md-link urls in place and
    // plan each wikilink's replacement. We defer tree surgery (detach/append) to a
    // second pass so we never mutate structure while iterating descendants.
    let mut wl_actions: Vec<(&AstNode, WikiAction)> = Vec::new();
    for node in root.descendants() {
        enum Kind {
            Link(Option<String>),
            Image(Option<String>),
            Wiki(WikiAction),
            None,
        }
        let kind = {
            let data = node.data.borrow();
            match &data.value {
                NodeValue::Link(link) => {
                    Kind::Link(rewrite_link(&link.url, page_dir, known_urls, policy)?)
                }
                NodeValue::Image(img) => Kind::Image(resolve_image(&img.url, page_dir, images)?),
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
            Kind::Image(Some(new_url)) => {
                let mut data = node.data.borrow_mut();
                if let NodeValue::Image(ref mut img) = data.value {
                    img.url = new_url;
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

/// What to do with one image url.
#[derive(Debug)]
pub enum ImageResolution {
    /// Leave the url exactly as the author wrote it.
    Keep,
    /// Replace the url with this one.
    Rewrite(String),
}

/// Resolves a page's relative image urls against whatever asset base that page
/// is rendered from — the docs tree for a normal page, the repo root for the
/// README/CLAUDE pages compositor surfaces from outside it (see compositor's
/// `root_assets`).
///
/// The traversal filters external / anchor / root-relative urls out before
/// calling this, so an implementation only ever sees a genuinely relative url.
///
/// Each implementation owns its own strictness: returning `Err` is how an
/// unresolvable image fails a strict `build`. That is deliberate — a repo-root
/// page renders its *links* leniently while still resolving its *images*
/// strictly, so one shared policy argument could not express both.
pub trait ImageResolver {
    fn resolve(&self, url: &str, page_dir: &Path) -> Result<ImageResolution>;
}

/// Validates a docs page's images against the asset set `build_site` collected
/// while walking the docs tree.
///
/// Resolution is validate-only: a docs asset is already mirrored into the output
/// by compositor's `copy_assets`, so the url the author wrote already resolves
/// and is kept as-is.
pub struct DocsAssets {
    /// Docs-dir-relative, `/`-separated.
    assets: HashSet<String>,
    policy: LinkPolicy,
}

impl DocsAssets {
    pub fn new(assets: HashSet<String>, policy: LinkPolicy) -> DocsAssets {
        DocsAssets { assets, policy }
    }
}

impl ImageResolver for DocsAssets {
    fn resolve(&self, url: &str, page_dir: &Path) -> Result<ImageResolution> {
        let key = normalize(&page_dir.join(url))
            .to_string_lossy()
            .replace('\\', "/");
        if self.assets.contains(&key) {
            return Ok(ImageResolution::Keep);
        }
        match self.policy {
            LinkPolicy::Strict => Err(anyhow!(
                "unresolvable image: {url} (from {})",
                page_dir.display()
            )),
            // Lenient: emit the dead src rather than abort an unattended
            // rebuild. It 404s in the browser — visible, not swallowed.
            LinkPolicy::Lenient => Ok(ImageResolution::Keep),
        }
    }
}

/// Urls the resolver never sees: absolute, protocol-relative, `data:`, anchors,
/// site-root-relative, and empty. These are not compositor's to resolve.
fn is_external_image_url(url: &str) -> bool {
    url.is_empty()
        || url.starts_with('#')
        || url.starts_with('/')
        || url.starts_with("data:")
        || url.contains("://")
}

fn resolve_image(url: &str, page_dir: &Path, images: &dyn ImageResolver) -> Result<Option<String>> {
    if is_external_image_url(url) {
        return Ok(None);
    }
    // Split off any trailing #fragment or ?query (mirrors rewrite_link's anchor
    // split above) so a resolver's lookup key is the bare path, then re-attach
    // it verbatim onto a Rewrite so the emitted url keeps it.
    let cut = url.find(['#', '?']).unwrap_or(url.len());
    let (path_part, suffix) = url.split_at(cut);
    // A Markdown author must percent-encode a filename with a space (or other
    // reserved character) for the url to parse at all; every resolver's lookup
    // key is a decoded filesystem-relative path, so decode before handing it
    // over. A path that isn't valid percent-encoded UTF-8 is passed through
    // as-is rather than failing resolution over an encoding quirk.
    let decoded = percent_encoding::percent_decode_str(path_part)
        .decode_utf8()
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| path_part.to_string());
    match images.resolve(&decoded, page_dir)? {
        ImageResolution::Keep => Ok(None),
        ImageResolution::Rewrite(u) => Ok(Some(format!("{u}{suffix}"))),
    }
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
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
            &no_images(),
        )
        .unwrap();
        // Comrak does not parse wikilinks inside code spans, so it stays literal text.
        assert!(r.html.contains("[[getting-started]]"), "got: {}", r.html);
        assert!(!r.html.contains("<a href"), "got: {}", r.html);
    }

    #[test]
    fn admonition_body_renders_as_markdown() {
        let known = HashSet::new();
        let r = render_markdown(
            "!!! note\n    This is **bold** text.\n",
            Path::new(""),
            &known,
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &no_images(),
        )
        .unwrap();
        assert!(
            r.html.contains("<div class=\"admonition note\">"),
            "{}",
            r.html
        );
        assert!(r.html.contains("<strong>bold</strong>"), "{}", r.html);
    }

    #[test]
    fn admonition_body_link_is_rewritten() {
        let mut known = HashSet::new();
        known.insert("cli/tar.html".to_string());
        let r = render_markdown(
            "!!! tip\n    See [tar](tar.md).\n",
            Path::new("cli"),
            &known,
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &no_images(),
        )
        .unwrap();
        assert!(
            r.html.contains("<div class=\"admonition tip\">")
                && r.html.contains("href=\"tar.html\""),
            "{}",
            r.html
        );
    }

    #[test]
    fn heading_inside_admonition_is_in_toc() {
        let r = render_markdown(
            "!!! note\n    ## Inner Heading\n\n    text\n",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &no_images(),
        )
        .unwrap();
        assert_eq!(r.toc.len(), 1);
        assert_eq!(r.toc[0].text, "Inner Heading");
        let id = &r.toc[0].id;
        assert!(r.html.contains(&format!("id=\"{id}\"")), "{}", r.html);
    }

    #[test]
    fn code_fence_inside_admonition_is_highlighted() {
        // A fenced code block in an admonition body still goes through syntect in
        // the single pass (the body renders as Markdown, not raw HTML).
        let r = render_markdown(
            "!!! note\n    ```rust\n    fn main() {}\n    ```\n",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &no_images(),
        )
        .unwrap();
        assert!(
            r.html.contains("<div class=\"admonition note\">"),
            "{}",
            r.html
        );
        // syntect emits inline styles on the highlighted tokens.
        assert!(
            r.html.contains("style=\"") && r.html.contains("main"),
            "{}",
            r.html
        );
    }

    fn no_images() -> DocsAssets {
        DocsAssets::new(HashSet::new(), LinkPolicy::Lenient)
    }

    fn docs_assets(paths: &[&str], policy: LinkPolicy) -> DocsAssets {
        DocsAssets::new(paths.iter().map(|s| s.to_string()).collect(), policy)
    }

    #[test]
    fn image_in_docs_assets_is_left_alone() {
        let images = docs_assets(&["img/shot.png"], LinkPolicy::Strict);
        let r = render_markdown(
            "![a](img/shot.png)",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &images,
        )
        .unwrap();
        assert!(r.html.contains(r#"src="img/shot.png""#), "got: {}", r.html);
    }

    #[test]
    fn image_resolves_relative_to_the_page_dir() {
        // A page at sub/page.md writing "img/shot.png" means sub/img/shot.png.
        let images = docs_assets(&["sub/img/shot.png"], LinkPolicy::Strict);
        let r = render_markdown(
            "![a](img/shot.png)",
            Path::new("sub"),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &images,
        )
        .unwrap();
        // The emitted src stays relative to the page, as rewrite_link does for links.
        assert!(r.html.contains(r#"src="img/shot.png""#), "got: {}", r.html);
    }

    #[test]
    fn missing_image_errors_under_strict() {
        let images = docs_assets(&[], LinkPolicy::Strict);
        let err = render_markdown(
            "![a](img/gone.png)",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &images,
        )
        .unwrap_err();
        assert!(err.to_string().contains("img/gone.png"), "got: {err}");
    }

    #[test]
    fn missing_image_degrades_under_lenient() {
        let images = docs_assets(&[], LinkPolicy::Lenient);
        let r = render_markdown(
            "![a](img/gone.png)",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Lenient,
            &images,
        )
        .unwrap();
        assert!(r.html.contains(r#"src="img/gone.png""#), "got: {}", r.html);
    }

    #[test]
    fn external_and_root_relative_images_are_never_validated() {
        // Badges in a README are absolute; a strict resolver with an empty asset set
        // must not touch or reject them.
        let images = docs_assets(&[], LinkPolicy::Strict);
        let r = render_markdown(
            "![c](https://img.shields.io/badge/x.svg)\n\n![d](data:image/png;base64,AA)\n\n![e](//cdn/x.png)\n\n![f](/root.png)",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &images,
        )
        .unwrap();
        assert!(
            r.html
                .contains(r#"src="https://img.shields.io/badge/x.svg""#),
            "got: {}",
            r.html
        );
        assert!(
            r.html.contains(r#"src="data:image/png;base64,AA""#),
            "got: {}",
            r.html
        );
        assert!(r.html.contains(r#"src="//cdn/x.png""#), "got: {}", r.html);
        assert!(r.html.contains(r#"src="/root.png""#), "got: {}", r.html);
    }

    #[test]
    fn image_url_with_fragment_resolves_and_keeps_its_fragment() {
        // sprite.svg#icon: the asset on disk is "sprite.svg"; the fragment must
        // survive into the emitted src unchanged, matching rewrite_link's anchor
        // handling for ordinary links.
        let images = docs_assets(&["sprite.svg"], LinkPolicy::Strict);
        let r = render_markdown(
            "![sprite](sprite.svg#icon)",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &images,
        )
        .unwrap();
        assert!(
            r.html.contains(r#"src="sprite.svg#icon""#),
            "got: {}",
            r.html
        );
    }

    #[test]
    fn image_url_with_query_resolves_and_keeps_its_query() {
        let images = docs_assets(&["sprite.svg"], LinkPolicy::Strict);
        let r = render_markdown(
            "![query](sprite.svg?v=1)",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &images,
        )
        .unwrap();
        assert!(
            r.html.contains(r#"src="sprite.svg?v=1""#),
            "got: {}",
            r.html
        );
    }

    #[test]
    fn percent_encoded_image_url_resolves_and_stays_encoded() {
        // Filenames with spaces are a live, acknowledged shape (see this repo's
        // CLAUDE.md); an author must percent-encode the space for Markdown to
        // parse the url at all, so the asset on disk is the decoded "my image.png".
        let images = docs_assets(&["my image.png"], LinkPolicy::Strict);
        let r = render_markdown(
            "![spaces](my%20image.png)",
            Path::new(""),
            &HashSet::new(),
            &WikiIndex::new(),
            LinkPolicy::Strict,
            &images,
        )
        .unwrap();
        assert!(
            r.html.contains(r#"src="my%20image.png""#),
            "the Keep path must not rewrite the author's url: {}",
            r.html
        );
    }
}
