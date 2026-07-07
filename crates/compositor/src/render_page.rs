use crate::config::SiteConfig;
use askama::Template;
use render_core::nav::{NavNode, NavTree};
use render_core::site::Page;

pub const STYLE_CSS: &str = include_str!("../assets/style.css");

#[derive(Template)]
#[template(path = "page.html")]
struct PageTemplate<'a> {
    page_title: &'a str,
    site_name: &'a str,
    style: &'a str,
    nav_html: String,
    body: &'a str,
}

pub fn render_page(cfg: &SiteConfig, nav: &NavTree, page: &Page) -> String {
    // Nav links are emitted site-root-relative (e.g. "cli/tar.html"), but the
    // page being rendered may live in a subdirectory, so the links must be
    // made relative to *this* page's location: one "../" per directory of
    // depth in the current page's own url.
    let depth = page.url.matches('/').count();
    let prefix = "../".repeat(depth);

    PageTemplate {
        page_title: &page.title,
        site_name: &cfg.site_name,
        style: STYLE_CSS,
        nav_html: nav_to_html(nav, &prefix),
        body: &page.html,
    }
    .render()
    .expect("template render is infallible")
}

fn nav_to_html(nav: &NavTree, prefix: &str) -> String {
    let mut s = String::from("<ul>");
    for node in &nav.0 {
        node_html(node, prefix, &mut s);
    }
    s.push_str("</ul>");
    s
}

fn node_html(node: &NavNode, prefix: &str, s: &mut String) {
    match node {
        NavNode::Page { title, url } => {
            s.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                html_escape(&format!("{prefix}{url}")),
                html_escape(title)
            ));
        }
        NavNode::Section { title, children } => {
            s.push_str(&format!(
                "<li class=\"section\"><span>{}</span><ul>",
                html_escape(title)
            ));
            for c in children {
                node_html(c, prefix, s);
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
