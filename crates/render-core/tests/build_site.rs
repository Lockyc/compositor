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

    let site = build_site(
        &tmp,
        render_core::LinkPolicy::Strict,
        &render_core::Excluder::new(&tmp, &tmp, &[]),
        false,
    )
    .unwrap();
    let by = |name: &str| {
        site.pages
            .iter()
            .find(|p| p.rel_path.ends_with(name))
            .unwrap()
    };

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

#[test]
fn wikilink_deep_link_lands_on_an_explicit_heading_id() {
    let tmp = std::env::temp_dir().join(format!("compositor-explicit-id-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    write(&tmp, "a.md", "# A\n\nSee [[Insurance#operating-set]].\n");
    write(
        &tmp,
        "insurance.md",
        "# Insurance\n\n### The operating set — three lines {#operating-set}\n",
    );

    let site = build_site(
        &tmp,
        render_core::LinkPolicy::Strict,
        &render_core::Excluder::new(&tmp, &tmp, &[]),
        false,
    )
    .unwrap();
    let by = |name: &str| {
        site.pages
            .iter()
            .find(|p| p.rel_path.ends_with(name))
            .unwrap()
    };

    assert!(
        by("a.md")
            .html
            .contains(r##"href="insurance.html#operating-set""##),
        "got: {}",
        by("a.md").html
    );
    assert!(by("insurance.md").html.contains(r#"id="operating-set""#));
    assert_eq!(by("insurance.md").toc[0].id, "operating-set");
    fs::remove_dir_all(&tmp).ok();
}
