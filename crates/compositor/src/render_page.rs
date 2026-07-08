use crate::config::SiteConfig;
use askama::Template;
use render_core::markdown::TocEntry;
use render_core::nav::{NavNode, NavTree};
use render_core::site::{Page, SiteModel};
use std::path::PathBuf;

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

/// Render the page's h2/h3 TOC as a nested list. h3s nest under the preceding
/// h2. Empty input yields an empty string (caller omits the aside entirely).
fn toc_to_html(toc: &[TocEntry]) -> String {
    if toc.is_empty() {
        return String::new();
    }
    let mut s = String::from("<ul>");
    let mut in_sub = false;
    for e in toc {
        let link = format!(
            "<li><a href=\"#{}\">{}</a></li>",
            html_escape(&e.id),
            html_escape(&e.text)
        );
        if e.level >= 3 {
            if !in_sub {
                s.push_str("<ul>");
                in_sub = true;
            }
            s.push_str(&link);
        } else {
            if in_sub {
                s.push_str("</ul>");
                in_sub = false;
            }
            s.push_str(&link);
        }
    }
    if in_sub {
        s.push_str("</ul>");
    }
    s.push_str("</ul>");
    s
}

/// Ensure the site has a home page served at `/` (`index.html`). compositor owns
/// the shell, so a docs tree with no landing page still gets a working `/`:
/// - a root `index.md` already produces `index.html` — nothing to add;
/// - otherwise a root `index`/`home`/`readme` file (any case) is promoted to the
///   home, aliased at `index.html` while keeping its own url so links still work;
/// - otherwise a blank home — the shell renders the navigation menu with an empty
///   body until a real landing page exists.
///
/// Returned (when `Some`) as an extra page for callers to render alongside the
/// real pages; it is intentionally not part of the nav tree.
pub fn resolve_home(site: &SiteModel) -> Option<Page> {
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
    Some(Page {
        url: "index.html".to_string(),
        rel_path: PathBuf::from("index.md"),
        title: "Home".to_string(),
        html: String::new(),
        toc: vec![],
    })
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

    #[test]
    fn no_home_added_when_index_exists() {
        let s = site(vec![page("index.md", "index.html", "x")]);
        assert!(resolve_home(&s).is_none());
    }

    #[test]
    fn readme_is_promoted_to_home_any_case() {
        let s = site(vec![page("README.md", "README.html", "readme body")]);
        let home = resolve_home(&s).unwrap();
        assert_eq!(home.url, "index.html");
        assert_eq!(home.html, "readme body");
    }

    #[test]
    fn home_md_beats_readme() {
        let s = site(vec![
            page("readme.md", "readme.html", "R"),
            page("home.md", "home.html", "H"),
        ]);
        assert_eq!(resolve_home(&s).unwrap().html, "H");
    }

    #[test]
    fn blank_home_when_no_landing_candidate() {
        let s = site(vec![page("guide.md", "guide.html", "g")]);
        let home = resolve_home(&s).unwrap();
        assert_eq!(home.url, "index.html");
        assert!(home.html.is_empty());
        assert_eq!(home.title, "Home");
    }

    #[test]
    fn nested_readme_is_not_promoted() {
        // A README inside a subdirectory must not become the site home.
        let s = site(vec![page("sub/readme.md", "sub/readme.html", "R")]);
        assert!(resolve_home(&s).unwrap().html.is_empty());
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
}
