use crate::config::SiteConfig;
use anyhow::Result;
use askama::Template;
use render_core::frontmatter::split_frontmatter;
use render_core::markdown::{render_markdown, ImageResolver, TocEntry};
use render_core::nav::{flatten, NavLink, NavNode, NavTree};
use render_core::site::{Page, SiteModel};
use render_core::LinkPolicy;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Template)]
#[template(path = "page.html")]
struct PageTemplate<'a> {
    page_title: &'a str,
    site_name: &'a str,
    home_href: String,
    asset_prefix: String,
    nav_html: String,
    toc_html: String,
    has_toc: bool,
    body: &'a str,
    has_prev: bool,
    prev_href: String,
    prev_title: &'a str,
    has_next: bool,
    next_href: String,
    next_title: &'a str,
}

/// Linear reading order for prev/next, single-sourced from the nav tree. The
/// landing page leads: a docs-root `index.md` is already the tree's first page,
/// but a *synthetic* home (repo README / generated index — see `resolve_home`)
/// isn't in the nav, so it's prepended here so `index.html` is always first.
pub fn reading_order(nav: &NavTree, home: Option<&Page>) -> Vec<NavLink> {
    let mut order = flatten(nav);
    if !order.iter().any(|l| l.url == "index.html") {
        if let Some(h) = home {
            order.insert(
                0,
                NavLink {
                    title: h.title.clone(),
                    url: "index.html".to_string(),
                },
            );
        }
    }
    order
}

/// The page before and after `url` in reading order (each `None` at an end, or
/// when the page isn't in the order — e.g. nothing to page through).
pub fn neighbours<'a>(
    order: &'a [NavLink],
    url: &str,
) -> (Option<&'a NavLink>, Option<&'a NavLink>) {
    match order.iter().position(|l| l.url == url) {
        Some(i) => (i.checked_sub(1).map(|j| &order[j]), order.get(i + 1)),
        None => (None, None),
    }
}

pub fn render_page(
    cfg: &SiteConfig,
    nav: &NavTree,
    page: &Page,
    prev: Option<&NavLink>,
    next: Option<&NavLink>,
) -> String {
    // Nav links are emitted site-root-relative (e.g. "cli/tar.html"), but the
    // page being rendered may live in a subdirectory, so the links must be
    // made relative to *this* page's location: one "../" per directory of
    // depth in the current page's own url.
    let depth = page.url.matches('/').count();
    let prefix = "../".repeat(depth);

    let toc_html = toc_to_html(&page.toc);
    let has_toc = !page.toc.is_empty();

    PageTemplate {
        page_title: &page.title,
        site_name: &cfg.site_name,
        // The site name links back to the home page (`/`), relative to this
        // page's depth (the menu also carries a "Home" entry, see `nav_to_html`).
        home_href: format!("{prefix}index.html"),
        asset_prefix: prefix.clone(),
        nav_html: nav_to_html(nav, &prefix, &page.url),
        toc_html,
        has_toc,
        body: &page.html,
        has_prev: prev.is_some(),
        prev_href: prev
            .map(|l| format!("{prefix}{}", l.url))
            .unwrap_or_default(),
        prev_title: prev.map(|l| l.title.as_str()).unwrap_or_default(),
        has_next: next.is_some(),
        next_href: next
            .map(|l| format!("{prefix}{}", l.url))
            .unwrap_or_default(),
        next_title: next.map(|l| l.title.as_str()).unwrap_or_default(),
    }
    .render()
    .expect("template render is infallible")
}

/// Render the page's h2/h3 TOC as a nested list: each h3 nests inside the
/// preceding h2's `<li>` (a proper `<ul>` sublist). An h3 with no preceding h2
/// (a page whose first sub-heading is an h3) renders at top level rather than
/// producing an `<li>`-less nested `<ul>`. Empty input yields an empty string
/// (the caller then omits the `<aside id="toc">` entirely).
fn toc_to_html(toc: &[TocEntry]) -> String {
    if toc.is_empty() {
        return String::new();
    }
    let mut s = String::from("<ul>");
    let mut open_li = false; // a top-level <li> is open, awaiting its close
    let mut in_sub = false; // a nested <ul> is currently open inside that <li>
    for e in toc {
        let link = format!(
            "<a href=\"#{}\">{}</a>",
            html_escape(&e.id),
            html_escape(&e.text)
        );
        if e.level >= 3 {
            if open_li {
                if !in_sub {
                    s.push_str("<ul>");
                    in_sub = true;
                }
                s.push_str(&format!("<li>{link}</li>"));
            } else {
                // Orphan h3: no parent h2, so render it at the top level.
                s.push_str(&format!("<li>{link}</li>"));
            }
        } else {
            if in_sub {
                s.push_str("</ul>");
                in_sub = false;
            }
            if open_li {
                s.push_str("</li>");
            }
            // Leave the <li> open so any following h3s nest inside it.
            s.push_str(&format!("<li>{link}"));
            open_li = true;
        }
    }
    if in_sub {
        s.push_str("</ul>");
    }
    if open_li {
        s.push_str("</li>");
    }
    s.push_str("</ul>");
    s
}

/// Ensure the site has a home page served at `/` (`index.html`). compositor owns
/// the shell, so a docs tree with no landing page still gets a working `/`. The
/// fallback chain, first match wins:
/// 1. a docs-tree root `index.md` already produces `index.html` — nothing to add;
/// 2. otherwise a docs-tree root `index`/`home`/`readme` (any case) is promoted,
///    aliased at `index.html` while keeping its own url so links still work;
/// 3. otherwise the **repo-root `README.md`** (any case), when the docs dir is a
///    subdir rather than the repo root itself — rendered as the landing page;
/// 4. otherwise a **generated index** — the site name over the nav as a link list
///    — so `/` is never a blank body.
///
/// The repo README (tier 3) is rendered with the *lenient* link policy for its
/// `.md` links: it is landing chrome sourced from outside the docs tree, so its
/// links don't share the docs tree's url base and must not be validated against
/// it (nor hard-fail a strict `build`). Its **images** are a different matter —
/// `images` carries the build's real policy, so an unresolvable one still fails
/// a strict `build` rather than shipping a silently broken `<img>`.
///
/// Returned (when `Some`) as an extra page for callers to render alongside the
/// real pages; it is intentionally not part of the nav tree.
pub fn resolve_home(
    site: &SiteModel,
    cfg: &SiteConfig,
    project_dir: &Path,
    images: &dyn ImageResolver,
) -> Result<Option<Page>> {
    if site.pages.iter().any(|p| p.url == "index.html") {
        return Ok(None);
    }
    for stem in ["index", "home", "readme"] {
        if let Some(src) = site.pages.iter().find(|p| is_root_named(p, stem)) {
            return Ok(Some(Page {
                url: "index.html".to_string(),
                rel_path: PathBuf::from("index.md"),
                title: src.title.clone(),
                html: src.html.clone(),
                toc: src.toc.clone(),
            }));
        }
    }
    // Tier 3 only applies when the docs dir is a subdir: when it *is* the repo
    // root (a bare Markdown folder), a root README is already a docs page and
    // was handled by the promotion loop above.
    if cfg.docs_path(project_dir) != project_dir {
        if let Some(home) = repo_readme_home(project_dir, &cfg.site_name, images)? {
            return Ok(Some(home));
        }
    }
    Ok(Some(generated_index(&cfg.site_name, &site.nav)))
}

/// Render the repo-root `README.md` (filename matched case-insensitively) into a
/// home page. `Ok(None)` when there is no README to read — the graceful default.
/// An `Err` means the README exists but did not render (e.g. an unresolvable
/// image under a strict `build`), which must surface rather than silently fall
/// through to the generated index.
fn repo_readme_home(
    project_dir: &Path,
    site_name: &str,
    images: &dyn ImageResolver,
) -> Result<Option<Page>> {
    let Some(path) = find_repo_root_md(project_dir, "readme") else {
        return Ok(None);
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let (fm, body) = split_frontmatter(&raw);
    // Empty known-urls + lenient *links*: the README is not part of the strict
    // docs link contract, and its link base differs from the docs tree's. Its
    // *images* are a different matter — they resolve against the repo root and
    // either exist on disk or don't, so `images` carries the build's real
    // policy and a dead one still fails a strict build.
    let rendered = render_markdown(
        &body,
        Path::new(""),
        &HashSet::new(),
        &render_core::wikilink::WikiIndex::new(),
        LinkPolicy::Lenient,
        images,
    )?;
    let title = fm
        .title
        .or(rendered.first_h1)
        .unwrap_or_else(|| site_name.to_string());
    Ok(Some(Page {
        url: "index.html".to_string(),
        rel_path: PathBuf::from("index.md"),
        title,
        html: rendered.html,
        toc: rendered.toc,
    }))
}

/// A top-level `*.md` file in `dir` whose stem equals `stem` (case-insensitive) —
/// the one discovery function for the repo-root files compositor surfaces from
/// outside the docs tree (`README.md` → home, `CLAUDE.md` → nav entry).
fn find_repo_root_md(dir: &Path, stem: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_file()
                && p.extension().and_then(|e| e.to_str()) == Some("md")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case(stem))
        })
}

/// Surface a **repo-root `CLAUDE.md`** as a top-level nav page (label `CLAUDE`),
/// adjacent to Home. The sibling of the repo-root README handling (`resolve_home`
/// tier 3), but as a *nav entry* rather than the home page.
///
/// A no-op unless the docs dir is a *subdir* (when it *is* the repo root, a `CLAUDE.md`
/// there is already an ordinary docs page in the nav), there is a repo-root `CLAUDE.md`
/// to read, and no page already occupies `CLAUDE.html` (a docs-tree `CLAUDE.md` is
/// already surfaced — don't double-add).
///
/// The label is the fixed stem `CLAUDE`, never content-derived: these files
/// conventionally open with a `# <projectname>` H1, which the normal title chain
/// would wrongly surface as the menu label.
///
/// Like the README, its `.md` links render leniently against an empty url/wiki
/// base — outside the docs link contract, so they degrade to honest 404s rather
/// than hard-failing a strict `build`. Its images follow `images`, the build's
/// real policy, so a dead one still fails a strict build instead of vanishing.
pub fn surface_repo_claude(
    site: &mut SiteModel,
    cfg: &SiteConfig,
    project_dir: &Path,
    images: &dyn ImageResolver,
) -> Result<()> {
    const URL: &str = "CLAUDE.html";
    // Docs dir *is* the repo root → a root CLAUDE.md is already a docs page.
    if cfg.docs_path(project_dir) == project_dir {
        return Ok(());
    }
    // Already surfaced (e.g. a docs-tree CLAUDE.md) → no duplicate entry.
    if site.pages.iter().any(|p| p.url == URL) {
        return Ok(());
    }
    let Some(path) = find_repo_root_md(project_dir, "claude") else {
        return Ok(());
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let (_fm, body) = split_frontmatter(&raw);
    let rendered = render_markdown(
        &body,
        Path::new(""),
        &HashSet::new(),
        &render_core::wikilink::WikiIndex::new(),
        LinkPolicy::Lenient,
        images,
    )?;
    site.pages.push(Page {
        url: URL.to_string(),
        rel_path: PathBuf::from("CLAUDE.md"),
        title: "CLAUDE".to_string(),
        html: rendered.html,
        toc: rendered.toc,
    });
    // Sit adjacent to Home: after a leading `index.html` page if the nav has one,
    // else first (the synthetic Home is rendered ahead of the nav list separately).
    let pos = usize::from(matches!(
        site.nav.0.first(),
        Some(NavNode::Page { url, .. }) if url.as_str() == "index.html"
    ));
    site.nav.0.insert(
        pos,
        NavNode::Page {
            title: "CLAUDE".to_string(),
            url: URL.to_string(),
        },
    );
    Ok(())
}

/// A generated landing page for a docs tree with no authored home: the site name
/// as an `<h1>`, followed by the nav rendered as a root-relative link list (the
/// same `nav_to_html` the sidebar uses — one source of truth for the markup).
fn generated_index(site_name: &str, nav: &NavTree) -> Page {
    let html = format!(
        "<h1>{}</h1>\n{}",
        html_escape(site_name),
        nav_to_html(nav, "", "index.html")
    );
    Page {
        url: "index.html".to_string(),
        rel_path: PathBuf::from("index.md"),
        title: site_name.to_string(),
        html,
        toc: vec![],
    }
}

/// A root-level (no subdirectory) page whose filename stem equals `stem`,
/// case-insensitively — so `README.md`, `Home.md`, etc. all match.
fn is_root_named(page: &Page, stem: &str) -> bool {
    let at_root = page
        .rel_path
        .parent()
        .is_none_or(|p| p.as_os_str().is_empty());
    let name_matches = page
        .rel_path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case(stem));
    at_root && name_matches
}

fn nav_to_html(nav: &NavTree, prefix: &str, current_url: &str) -> String {
    let mut s = String::from("<ul>");
    // The menu leads with a "Home" link back to `/` — unless a real page already
    // occupies index.html (a docs-root index.md), which is itself the home and
    // already leads the tree, so a synthetic entry would just duplicate it.
    let has_index_page = nav
        .0
        .iter()
        .any(|n| matches!(n, NavNode::Page { url, .. } if url == "index.html"));
    if !has_index_page {
        let current = if current_url == "index.html" {
            " aria-current=\"page\""
        } else {
            ""
        };
        s.push_str(&format!(
            "<li><a href=\"{}\"{current}>Home</a></li>",
            html_escape(&format!("{prefix}index.html"))
        ));
    }
    for node in &nav.0 {
        node_html(node, prefix, current_url, &mut s);
    }
    s.push_str("</ul>");
    s
}

fn node_html(node: &NavNode, prefix: &str, current_url: &str, s: &mut String) {
    match node {
        NavNode::Page { title, url } => {
            let current = if url == current_url {
                " aria-current=\"page\""
            } else {
                ""
            };
            s.push_str(&format!(
                "<li><a href=\"{}\"{}>{}</a></li>",
                html_escape(&format!("{prefix}{url}")),
                current,
                html_escape(title)
            ));
        }
        NavNode::Section { title, children } => {
            s.push_str(&format!(
                "<li class=\"section\"><span>{}</span><ul>",
                html_escape(title)
            ));
            for c in children {
                node_html(c, prefix, current_url, s);
            }
            s.push_str("</ul></li>");
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(rel: &str, url: &str, html: &str) -> Page {
        Page {
            rel_path: PathBuf::from(rel),
            url: url.to_string(),
            title: "T".to_string(),
            html: html.to_string(),
            toc: vec![],
        }
    }

    fn site(pages: Vec<Page>) -> SiteModel {
        SiteModel {
            pages,
            nav: NavTree(vec![]),
        }
    }

    fn cfg_named(name: &str) -> SiteConfig {
        SiteConfig {
            site_name: name.to_string(),
            docs_dir: Some("docs".to_string()),
            ..SiteConfig::default()
        }
    }

    /// A fresh temp project dir with a `docs/` subdir (so `docs_path` differs
    /// from the project root and the repo-root README tier is reachable).
    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("compositor-home-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("docs")).unwrap();
        p
    }

    /// A permissive resolver for tests not concerned with image handling —
    /// nothing under it is ever expected to render an `<img>`.
    fn no_images() -> render_core::DocsAssets {
        render_core::DocsAssets::new(HashSet::new(), LinkPolicy::Lenient)
    }

    #[test]
    fn no_home_added_when_index_exists() {
        let s = site(vec![page("index.md", "index.html", "x")]);
        // Short-circuits before touching the filesystem, so any path works.
        assert!(
            resolve_home(&s, &cfg_named("S"), Path::new("/nonexistent"), &no_images())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn docs_tree_readme_is_promoted_to_home_any_case() {
        let s = site(vec![page("README.md", "README.html", "readme body")]);
        let home = resolve_home(&s, &cfg_named("S"), Path::new("/nonexistent"), &no_images())
            .unwrap()
            .unwrap();
        assert_eq!(home.url, "index.html");
        assert_eq!(home.html, "readme body");
    }

    #[test]
    fn docs_tree_home_md_beats_readme() {
        let s = site(vec![
            page("readme.md", "readme.html", "R"),
            page("home.md", "home.html", "H"),
        ]);
        assert_eq!(
            resolve_home(&s, &cfg_named("S"), Path::new("/nonexistent"), &no_images())
                .unwrap()
                .unwrap()
                .html,
            "H"
        );
    }

    #[test]
    fn repo_root_readme_becomes_home_when_docs_is_a_subdir() {
        let tmp = scratch("repo-readme");
        std::fs::write(tmp.join("README.md"), "# Welcome\n\nhello world").unwrap();
        let s = site(vec![page("guide.md", "guide.html", "g")]);
        let home = resolve_home(&s, &cfg_named("S"), &tmp, &no_images())
            .unwrap()
            .unwrap();
        assert_eq!(home.url, "index.html");
        assert_eq!(home.title, "Welcome");
        assert!(home.html.contains("hello world"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn repo_root_readme_found_case_insensitively() {
        let tmp = scratch("repo-readme-case");
        std::fs::write(tmp.join("readme.md"), "# Casing\n\nbody").unwrap();
        let s = site(vec![page("guide.md", "guide.html", "g")]);
        let home = resolve_home(&s, &cfg_named("S"), &tmp, &no_images())
            .unwrap()
            .unwrap();
        assert_eq!(home.title, "Casing");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn docs_tree_candidate_beats_repo_root_readme() {
        // A purpose-authored docs landing wins over the repo-root README.
        let tmp = scratch("docs-beats-repo");
        std::fs::write(tmp.join("README.md"), "# Repo\n\nrepo body").unwrap();
        let s = site(vec![page("home.md", "home.html", "docs home body")]);
        let home = resolve_home(&s, &cfg_named("S"), &tmp, &no_images())
            .unwrap()
            .unwrap();
        assert_eq!(home.html, "docs home body");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn generated_index_when_no_landing_candidate() {
        // No docs landing and no repo-root README -> a generated index, not blank.
        let tmp = scratch("gen-index");
        let s = site(vec![page("guide.md", "guide.html", "g")]);
        let home = resolve_home(&s, &cfg_named("My Site"), &tmp, &no_images())
            .unwrap()
            .unwrap();
        assert_eq!(home.url, "index.html");
        assert_eq!(home.title, "My Site");
        assert!(home.html.contains("<h1>My Site</h1>"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn nested_readme_is_not_promoted() {
        // A README inside a subdirectory must not become the site home; with no
        // repo-root README either, the home is the generated index.
        let tmp = scratch("nested-readme");
        let s = site(vec![page("sub/readme.md", "sub/readme.html", "R")]);
        let home = resolve_home(&s, &cfg_named("S"), &tmp, &no_images())
            .unwrap()
            .unwrap();
        assert!(!home.html.contains("R"));
        assert!(home.html.contains("<h1>S</h1>"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn repo_root_claude_md_surfaced_as_nav_page_when_docs_is_a_subdir() {
        let tmp = scratch("repo-claude");
        // Opens with an H1 that is the project name — the label must NOT derive
        // from it, or the menu would read "compositor" instead of "CLAUDE".
        std::fs::write(tmp.join("CLAUDE.md"), "# compositor\n\nagent notes").unwrap();
        let mut s = site(vec![page("guide.md", "guide.html", "g")]);
        surface_repo_claude(&mut s, &cfg_named("S"), &tmp, &no_images()).unwrap();

        let p = s
            .pages
            .iter()
            .find(|p| p.url == "CLAUDE.html")
            .expect("CLAUDE page added");
        assert_eq!(p.title, "CLAUDE", "label is the fixed stem, not the H1");
        assert!(p.html.contains("agent notes"), "body rendered: {}", p.html);

        assert!(
            s.nav.0.iter().any(|n| matches!(
                n,
                NavNode::Page { url, title } if url == "CLAUDE.html" && title == "CLAUDE"
            )),
            "nav gained a CLAUDE entry"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn repo_root_claude_found_case_insensitively() {
        let tmp = scratch("repo-claude-case");
        std::fs::write(tmp.join("claude.md"), "lower body").unwrap();
        let mut s = site(vec![page("guide.md", "guide.html", "g")]);
        surface_repo_claude(&mut s, &cfg_named("S"), &tmp, &no_images()).unwrap();
        assert!(s.pages.iter().any(|p| p.url == "CLAUDE.html"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn no_claude_surfaced_when_absent() {
        let tmp = scratch("repo-no-claude");
        let mut s = site(vec![page("guide.md", "guide.html", "g")]);
        surface_repo_claude(&mut s, &cfg_named("S"), &tmp, &no_images()).unwrap();
        assert!(!s.pages.iter().any(|p| p.url == "CLAUDE.html"));
        assert!(s.nav.0.is_empty());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn docs_tree_claude_is_not_double_added() {
        // A CLAUDE.md already inside the docs tree is already a page/nav entry;
        // the repo-root pass must not add a second CLAUDE.html.
        let tmp = scratch("repo-claude-dup");
        std::fs::write(tmp.join("CLAUDE.md"), "repo root claude").unwrap();
        let mut s = site(vec![page("CLAUDE.md", "CLAUDE.html", "docs-tree claude")]);
        surface_repo_claude(&mut s, &cfg_named("S"), &tmp, &no_images()).unwrap();
        let claude: Vec<_> = s.pages.iter().filter(|p| p.url == "CLAUDE.html").collect();
        assert_eq!(claude.len(), 1, "no duplicate CLAUDE.html");
        assert_eq!(claude[0].html, "docs-tree claude", "docs-tree page kept");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn claude_nav_entry_sits_after_a_leading_index_page() {
        let tmp = scratch("repo-claude-order");
        std::fs::write(tmp.join("CLAUDE.md"), "notes").unwrap();
        let mut s = site(vec![page("guide.md", "guide.html", "g")]);
        // Nav leads with a real index page (as a docs-root index.md would).
        s.nav.0.insert(
            0,
            NavNode::Page {
                title: "Welcome".into(),
                url: "index.html".into(),
            },
        );
        surface_repo_claude(&mut s, &cfg_named("S"), &tmp, &no_images()).unwrap();
        // Order: index.html, then CLAUDE.html.
        match (&s.nav.0[0], &s.nav.0[1]) {
            (NavNode::Page { url: u0, .. }, NavNode::Page { url: u1, .. }) => {
                assert_eq!(u0, "index.html");
                assert_eq!(u1, "CLAUDE.html");
            }
            _ => panic!("unexpected nav shape: {} nodes", s.nav.0.len()),
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn claude_not_surfaced_when_docs_is_the_repo_root() {
        // When docs_dir is ".", a repo-root CLAUDE.md is already a docs page; the
        // repo-root pass must not add it again.
        let tmp = scratch("repo-claude-flat");
        std::fs::write(tmp.join("CLAUDE.md"), "notes").unwrap();
        let cfg = SiteConfig {
            site_name: "S".to_string(),
            docs_dir: Some(".".to_string()),
            ..SiteConfig::default()
        };
        let mut s = site(vec![page("guide.md", "guide.html", "g")]);
        surface_repo_claude(&mut s, &cfg, &tmp, &no_images()).unwrap();
        assert!(!s.pages.iter().any(|p| p.url == "CLAUDE.html"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn menu_includes_home_link_when_no_index_page() {
        // A synthetic home (repo README / generated index) is not a nav page, so
        // the menu gains an explicit "Home" entry linking back to `/`.
        let cfg = cfg_named("S");
        let nav = NavTree(vec![NavNode::Page {
            title: "Guide".into(),
            url: "guide.html".into(),
        }]);
        let out = render_page(
            &cfg,
            &nav,
            &page("guide.md", "guide.html", "<p>x</p>"),
            None,
            None,
        );
        assert!(
            out.contains(r#"<li><a href="index.html">Home</a></li>"#),
            "{out}"
        );
    }

    #[test]
    fn menu_home_is_marked_current_on_the_home_page() {
        let cfg = cfg_named("S");
        let nav = NavTree(vec![NavNode::Page {
            title: "Guide".into(),
            url: "guide.html".into(),
        }]);
        let out = render_page(
            &cfg,
            &nav,
            &page("index.md", "index.html", "<p>home</p>"),
            None,
            None,
        );
        assert!(
            out.contains(r#"<li><a href="index.html" aria-current="page">Home</a></li>"#),
            "{out}"
        );
    }

    #[test]
    fn menu_omits_synthetic_home_when_a_real_index_page_exists() {
        // A docs-root index.md already occupies index.html and leads the menu, so
        // no duplicate synthetic "Home" is added.
        let cfg = cfg_named("S");
        let nav = NavTree(vec![
            NavNode::Page {
                title: "Welcome".into(),
                url: "index.html".into(),
            },
            NavNode::Page {
                title: "Guide".into(),
                url: "guide.html".into(),
            },
        ]);
        let out = render_page(
            &cfg,
            &nav,
            &page("guide.md", "guide.html", "<p>x</p>"),
            None,
            None,
        );
        assert!(!out.contains(">Home</a>"), "{out}");
        assert!(out.contains(">Welcome</a>"));
    }

    fn page_with_toc(url: &str, html: &str, toc: Vec<render_core::markdown::TocEntry>) -> Page {
        Page {
            rel_path: PathBuf::from(url.replace(".html", ".md")),
            url: url.to_string(),
            title: "T".to_string(),
            html: html.to_string(),
            toc,
        }
    }

    #[test]
    fn renders_topbar_and_toc_when_headings_present() {
        let cfg = SiteConfig {
            site_name: "S".into(),
            ..SiteConfig::default()
        };
        let toc = vec![render_core::markdown::TocEntry {
            level: 2,
            id: "alpha".into(),
            text: "Alpha".into(),
        }];
        let p = page_with_toc("guide.html", "<h2 id=\"alpha\">Alpha</h2>", toc);
        let out = render_page(&cfg, &NavTree(vec![]), &p, None, None);
        assert!(out.contains("class=\"topbar\""));
        assert!(out.contains("theme-toggle"));
        assert!(out.contains("id=\"toc\""));
        assert!(out.contains("href=\"#alpha\""));
        assert!(out.contains("has-toc"));
    }

    #[test]
    fn omits_toc_aside_when_no_headings() {
        let cfg = SiteConfig {
            site_name: "S".into(),
            ..SiteConfig::default()
        };
        let p = page_with_toc("guide.html", "<p>flat</p>", vec![]);
        let out = render_page(&cfg, &NavTree(vec![]), &p, None, None);
        assert!(!out.contains("id=\"toc\""));
        assert!(!out.contains("has-toc"));
    }

    #[test]
    fn toc_nests_h3_inside_preceding_h2_li() {
        let toc = vec![
            render_core::markdown::TocEntry {
                level: 2,
                id: "alpha".into(),
                text: "Alpha".into(),
            },
            render_core::markdown::TocEntry {
                level: 3,
                id: "beta".into(),
                text: "Beta".into(),
            },
            render_core::markdown::TocEntry {
                level: 3,
                id: "gamma".into(),
                text: "Gamma".into(),
            },
            render_core::markdown::TocEntry {
                level: 2,
                id: "delta".into(),
                text: "Delta".into(),
            },
        ];
        assert_eq!(
            toc_to_html(&toc),
            "<ul><li><a href=\"#alpha\">Alpha</a><ul><li><a href=\"#beta\">Beta</a></li><li><a href=\"#gamma\">Gamma</a></li></ul></li><li><a href=\"#delta\">Delta</a></li></ul>"
        );
    }

    #[test]
    fn toc_leading_h3_renders_at_top_level_not_orphan_sublist() {
        let toc = vec![
            render_core::markdown::TocEntry {
                level: 3,
                id: "sub".into(),
                text: "Sub".into(),
            },
            render_core::markdown::TocEntry {
                level: 2,
                id: "top".into(),
                text: "Top".into(),
            },
        ];
        let html = toc_to_html(&toc);
        assert!(
            !html.contains("<ul><ul>"),
            "orphan h3 produced an <li>-less nested <ul>: {html}"
        );
        assert_eq!(
            html,
            "<ul><li><a href=\"#sub\">Sub</a></li><li><a href=\"#top\">Top</a></li></ul>"
        );
    }

    fn link(title: &str, url: &str) -> NavLink {
        NavLink {
            title: title.into(),
            url: url.into(),
        }
    }

    #[test]
    fn page_nav_renders_prev_and_next_with_depth_relative_hrefs() {
        let cfg = SiteConfig {
            site_name: "S".into(),
            ..SiteConfig::default()
        };
        let prev = link("Alpha", "topics/alpha.html");
        let next = link("Gamma", "topics/gamma.html");
        // A page one directory deep: hrefs must be rewritten relative to it.
        let p = page("topics/beta.md", "topics/beta.html", "<p>b</p>");
        let out = render_page(&cfg, &NavTree(vec![]), &p, Some(&prev), Some(&next));
        assert!(out.contains("class=\"page-nav\""), "{out}");
        assert!(out.contains(r#"href="../topics/alpha.html""#), "{out}");
        assert!(out.contains(">Alpha</span>"), "{out}");
        assert!(out.contains(r#"href="../topics/gamma.html""#), "{out}");
        assert!(out.contains(">Gamma</span>"), "{out}");
        assert!(out.contains("← Previous"));
        assert!(out.contains("Next →"));
    }

    #[test]
    fn page_nav_omitted_when_no_neighbours() {
        let cfg = SiteConfig {
            site_name: "S".into(),
            ..SiteConfig::default()
        };
        let out = render_page(
            &cfg,
            &NavTree(vec![]),
            &page("only.md", "only.html", "<p>x</p>"),
            None,
            None,
        );
        assert!(!out.contains("class=\"page-nav\""), "{out}");
    }

    #[test]
    fn footer_attribution_is_present() {
        let cfg = SiteConfig {
            site_name: "S".into(),
            ..SiteConfig::default()
        };
        let out = render_page(
            &cfg,
            &NavTree(vec![]),
            &page("a.md", "a.html", "<p>x</p>"),
            None,
            None,
        );
        assert!(out.contains("class=\"site-footer\""), "{out}");
        assert!(out.contains("Built with"), "{out}");
    }

    #[test]
    fn reading_order_prepends_synthetic_home() {
        // Nav has no index.html (synthetic home from resolve_home). The home
        // page is prepended so index.html leads the reading order.
        let nav = NavTree(vec![NavNode::Page {
            title: "Guide".into(),
            url: "guide.html".into(),
        }]);
        let home = page("index.md", "index.html", "home");
        let order = reading_order(&nav, Some(&home));
        let urls: Vec<&str> = order.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["index.html", "guide.html"]);
    }

    #[test]
    fn reading_order_keeps_real_index_first_without_duplicating() {
        // A docs-root index.md is already index.html in the nav; it must not be
        // duplicated by the synthetic-home prepend.
        let nav = NavTree(vec![
            NavNode::Page {
                title: "Welcome".into(),
                url: "index.html".into(),
            },
            NavNode::Page {
                title: "Guide".into(),
                url: "guide.html".into(),
            },
        ]);
        let order = reading_order(&nav, None);
        let urls: Vec<&str> = order.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["index.html", "guide.html"]);
    }

    #[test]
    fn neighbours_reports_ends_and_middle() {
        let order = vec![
            link("H", "index.html"),
            link("A", "a.html"),
            link("B", "b.html"),
        ];
        let (p, n) = neighbours(&order, "index.html");
        assert!(p.is_none());
        assert_eq!(n.unwrap().url, "a.html");
        let (p, n) = neighbours(&order, "a.html");
        assert_eq!(p.unwrap().url, "index.html");
        assert_eq!(n.unwrap().url, "b.html");
        let (p, n) = neighbours(&order, "b.html");
        assert_eq!(p.unwrap().url, "a.html");
        assert!(n.is_none());
        let (p, n) = neighbours(&order, "missing.html");
        assert!(p.is_none() && n.is_none());
    }
}
