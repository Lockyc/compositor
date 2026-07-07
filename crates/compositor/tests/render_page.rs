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

#[test]
fn nav_url_and_title_are_escaped_for_html_attribute_context() {
    let cfg = SiteConfig { site_name: "Cheatsheet".into(), ..Default::default() };
    let nav = NavTree(vec![NavNode::Page {
        title: "Weird <Title>".into(),
        url: "a\".x.html".into(),
    }]);
    let page = Page {
        rel_path: PathBuf::from("index.md"),
        url: "index.html".into(),
        title: "Home".into(),
        html: "<p>hello</p>".into(),
    };
    let out = render_page(&cfg, &nav, &page);

    // The raw quote must not be allowed to break out of the href attribute.
    assert!(!out.contains("href=\"a\".x.html\""));
    assert!(out.contains("href=\"a&quot;.x.html\""));

    // The raw angle brackets in the title must not be allowed to inject markup.
    assert!(!out.contains("<Title>"));
    assert!(out.contains("Weird &lt;Title&gt;"));
}
