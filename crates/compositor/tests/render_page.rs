use compositor::config::SiteConfig;
use compositor::render_page::render_page;
use render_core::nav::{NavNode, NavTree};
use render_core::site::Page;
use std::path::PathBuf;

#[test]
fn page_html_has_title_body_and_nav() {
    let cfg = SiteConfig { site_name: "Cheatsheet".into(), ..Default::default() };
    let nav = NavTree(vec![NavNode::Page { title: "Home".into(), url: "index.html".into() }]);
    let page = Page {
        rel_path: PathBuf::from("index.md"),
        url: "index.html".into(),
        title: "Home".into(),
        html: "<p>hello</p>".into(),
    };
    let out = render_page(&cfg, &nav, &page);
    assert!(out.contains("<title>Home · Cheatsheet</title>"));
    assert!(out.contains("<p>hello</p>"));
    assert!(out.contains("href=\"index.html\""));
    assert!(out.contains("Cheatsheet"));
}
