use render_core::site::{build_site, humanize_filename};
use std::fs;

fn write(dir: &std::path::Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

#[test]
fn title_prefers_frontmatter_then_h1_then_filename() {
    let tmp = std::env::temp_dir().join(format!("compositor-t4-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    write(&tmp, "a.md", "---\ntitle: From FM\n---\n# Ignored H1\n");
    write(&tmp, "b.md", "# From Heading\n\nx");
    write(&tmp, "cli/git-repos.md", "plain body, no heading");

    let site = build_site(&tmp).unwrap();
    let by = |name: &str| site.pages.iter().find(|p| p.rel_path.ends_with(name)).unwrap();

    assert_eq!(by("a.md").title, "From FM");
    assert_eq!(by("b.md").title, "From Heading");
    assert_eq!(by("git-repos.md").title, "Git Repos");
    assert_eq!(by("a.md").url, "a.html");
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn humanize_examples() {
    assert_eq!(humanize_filename("git-repos"), "Git Repos");
    assert_eq!(humanize_filename("index"), "Index");
}
