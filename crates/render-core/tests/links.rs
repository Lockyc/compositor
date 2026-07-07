use render_core::markdown::render_markdown;
use std::collections::HashSet;
use std::path::Path;

fn urls(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn rewrites_relative_md_link_to_html() {
    let known = urls(&["cli/tar.html"]);
    let r = render_markdown("[tar](tar.md)", Path::new("cli"), &known).unwrap();
    assert!(r.html.contains("href=\"tar.html\""));
}

#[test]
fn leaves_external_links_untouched() {
    let known = urls(&[]);
    let r = render_markdown("[x](https://example.com)", Path::new(""), &known).unwrap();
    assert!(r.html.contains("href=\"https://example.com\""));
}

#[test]
fn errors_on_unresolvable_internal_link() {
    let known = urls(&["cli/tar.html"]);
    let err = render_markdown("[gone](missing.md)", Path::new("cli"), &known);
    assert!(err.is_err());
}
