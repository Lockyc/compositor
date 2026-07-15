use crate::config::SiteConfig;
use askama::Template;
use render_core::frontmatter::split_frontmatter;
use render_core::markdown::{render_markdown, TocEntry};
use render_core::nav::{NavNode, NavTree};
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
}

pub fn render_page(cfg: &SiteConfig, nav: &NavTree, page: &Page) -> String {
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
        // page's depth — the only always-present way back to `/`.
        home_href: format!("{prefix}index.html"),
        asset_prefix: prefix.clone(),
        nav_html: nav_to_html(nav, &prefix, &page.url),
        toc_html,
        has_toc,
        body: &page.html,
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
/// The repo README (tier 3) is rendered with the *lenient* link policy: it is
/// landing chrome sourced from outside the docs tree, so its links don't share
/// the docs tree's url base and must not be validated against it (nor hard-fail a
/// strict `build`).
///
/// Returned (when `Some`) as an extra page for callers to render alongside the
/// real pages; it is intentionally not part of the nav tree.
pub fn resolve_home(site: &SiteModel, cfg: &SiteConfig, project_dir: &Path) -> Option<Page> {
    if site.pages.iter().any(|p| p.url == "index.html") {
        return None;
    }
    for stem in ["index", "home", "readme"] {
        if let Some(src) = site.pages.iter().find(|p| is_root_named(p, stem)) {
            return Some(Page {
                url: "index.html".to_string(),
                rel_path: PathBuf::from("index.md"),
                title: src.title.clone(),
                html: src.html.clone(),
                toc: src.toc.clone(),
            });
        }
    }
    // Tier 3 only applies when the docs dir is a subdir: when it *is* the repo
    // root (a bare Markdown folder), a root README is already a docs page and
    // was handled by the promotion loop above.
    if cfg.docs_path(project_dir) != project_dir {
        if let Some(home) = repo_readme_home(project_dir, &cfg.site_name) {
            return Some(home);
        }
    }
    Some(generated_index(&cfg.site_name, &site.nav))
}

/// Render the repo-root `README.md` (filename matched case-insensitively) into a
/// home page, or `None` if there is no README to read/render.
fn repo_readme_home(project_dir: &Path, site_name: &str) -> Option<Page> {
    let path = find_repo_readme(project_dir)?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let (fm, body) = split_frontmatter(&raw);
    // Empty known-urls + lenient: the README is not part of the strict docs link
    // contract, and its link base differs from the docs tree's.
    let rendered =
        render_markdown(&body, Path::new(""), &HashSet::new(), LinkPolicy::Lenient).ok()?;
    let title = fm
        .title
        .or(rendered.first_h1)
        .unwrap_or_else(|| site_name.to_string());
    Some(Page {
        url: "index.html".to_string(),
        rel_path: PathBuf::from("index.md"),
        title,
        html: rendered.html,
        toc: rendered.toc,
    })
}

/// A top-level `*.md` file in `dir` whose stem is `readme` (case-insensitive).
fn find_repo_readme(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
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

    #[test]
    fn no_home_added_when_index_exists() {
        let s = site(vec![page("index.md", "index.html", "x")]);
        // Short-circuits before touching the filesystem, so any path works.
        assert!(resolve_home(&s, &cfg_named("S"), Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn docs_tree_readme_is_promoted_to_home_any_case() {
        let s = site(vec![page("README.md", "README.html", "readme body")]);
        let home = resolve_home(&s, &cfg_named("S"), Path::new("/nonexistent")).unwrap();
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
            resolve_home(&s, &cfg_named("S"), Path::new("/nonexistent"))
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
        let home = resolve_home(&s, &cfg_named("S"), &tmp).unwrap();
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
        let home = resolve_home(&s, &cfg_named("S"), &tmp).unwrap();
        assert_eq!(home.title, "Casing");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn docs_tree_candidate_beats_repo_root_readme() {
        // A purpose-authored docs landing wins over the repo-root README.
        let tmp = scratch("docs-beats-repo");
        std::fs::write(tmp.join("README.md"), "# Repo\n\nrepo body").unwrap();
        let s = site(vec![page("home.md", "home.html", "docs home body")]);
        let home = resolve_home(&s, &cfg_named("S"), &tmp).unwrap();
        assert_eq!(home.html, "docs home body");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn generated_index_when_no_landing_candidate() {
        // No docs landing and no repo-root README -> a generated index, not blank.
        let tmp = scratch("gen-index");
        let s = site(vec![page("guide.md", "guide.html", "g")]);
        let home = resolve_home(&s, &cfg_named("My Site"), &tmp).unwrap();
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
        let home = resolve_home(&s, &cfg_named("S"), &tmp).unwrap();
        assert!(!home.html.contains("R"));
        assert!(home.html.contains("<h1>S</h1>"));
        std::fs::remove_dir_all(&tmp).ok();
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
        let out = render_page(&cfg, &NavTree(vec![]), &p);
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
        let out = render_page(&cfg, &NavTree(vec![]), &p);
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
}
