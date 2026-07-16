use compositor::build::run_build;
use render_core::LinkPolicy;
use std::fs;

#[test]
fn build_writes_html_files() {
    let tmp = std::env::temp_dir().join(format!("compositor-build-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs/cli")).unwrap();
    fs::write(tmp.join("compositor.toml"), "site_name = \"Test\"\n").unwrap();
    fs::write(tmp.join("docs/index.md"), "# Home\n\n[tar](cli/tar.md)").unwrap();
    fs::write(tmp.join("docs/cli/tar.md"), "# Tar\n\nbody").unwrap();

    run_build(&tmp, LinkPolicy::Strict).unwrap();

    let index = fs::read_to_string(tmp.join("site/index.html")).unwrap();
    assert!(index.contains("<title>Home · Test</title>"));
    assert!(index.contains("href=\"cli/tar.html\""));
    assert!(tmp.join("site/cli/tar.html").exists());
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_copies_non_markdown_assets_into_the_site() {
    let tmp = std::env::temp_dir().join(format!("compositor-build-assets-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs/img")).unwrap();
    fs::write(tmp.join("compositor.toml"), "site_name = \"Test\"\n").unwrap();
    // A page that references an image, plus the image itself and a downloadable
    // asset. Without copying, the rendered <img>/link would 404 in the site.
    fs::write(tmp.join("docs/index.md"), "# Home\n\n![logo](img/logo.png)").unwrap();
    fs::write(tmp.join("docs/img/logo.png"), b"\x89PNG fake bytes").unwrap();
    fs::write(tmp.join("docs/data.csv"), "a,b\n1,2\n").unwrap();

    run_build(&tmp, LinkPolicy::Strict).unwrap();

    // The image is copied verbatim (bytes preserved) at its mirrored path.
    assert_eq!(
        fs::read(tmp.join("site/img/logo.png")).unwrap(),
        b"\x89PNG fake bytes"
    );
    // Any non-.md file is copied, not just images.
    assert!(tmp.join("site/data.csv").exists());
    // Markdown is rendered to HTML, never copied verbatim.
    assert!(!tmp.join("site/index.md").exists());
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_rejects_out_dir_that_would_delete_the_project() {
    let tmp = std::env::temp_dir().join(format!("compositor-build-outdir-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs")).unwrap();
    // A misconfigured out_dir of "." normalizes to the project dir itself;
    // remove_dir_all(out) on it would wipe the source tree (including docs/).
    fs::write(
        tmp.join("compositor.toml"),
        "site_name = \"Test\"\nout_dir = \".\"\n",
    )
    .unwrap();
    fs::write(tmp.join("docs/index.md"), "# Home").unwrap();

    let result = run_build(&tmp, LinkPolicy::Strict);

    assert!(result.is_err(), "run_build must reject out_dir = \".\"");
    assert!(
        tmp.join("docs/index.md").exists(),
        "the project's docs/ must survive a rejected build, not be deleted"
    );
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_works_without_compositor_toml_using_docs_subdir() {
    let tmp = std::env::temp_dir().join(format!("compositor-build-notoml-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs")).unwrap();
    // No compositor.toml at all — defaults are synthesized (site_name from the
    // folder, docs_dir = "docs" since that subdir exists).
    fs::write(tmp.join("docs/index.md"), "# Home\n\nbody").unwrap();

    run_build(&tmp, LinkPolicy::Strict).unwrap();

    let index = fs::read_to_string(tmp.join("site/index.html")).unwrap();
    // Page rendered; title is "<h1> · <derived site_name>".
    assert!(index.contains("<title>Home"), "index: {index}");
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_synthesizes_a_home_when_no_index_md() {
    let tmp = std::env::temp_dir().join(format!("compositor-build-home-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs")).unwrap();
    fs::write(tmp.join("compositor.toml"), "site_name = \"S\"\n").unwrap();
    // No index.md / home.md / readme.md — only a content page.
    fs::write(tmp.join("docs/guide.md"), "# Guide\n\nbody").unwrap();

    run_build(&tmp, LinkPolicy::Strict).unwrap();

    // A home page is generated at index.html; the shell carries the nav menu,
    // so the guide is reachable from `/` even with no landing file.
    let index = fs::read_to_string(tmp.join("site/index.html")).unwrap();
    assert!(
        index.contains(">Guide</a>"),
        "home should list the menu: {index}"
    );
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_promotes_readme_to_the_home() {
    let tmp = std::env::temp_dir().join(format!("compositor-build-readme-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs")).unwrap();
    fs::write(tmp.join("compositor.toml"), "site_name = \"S\"\n").unwrap();
    // A root README (uppercase) and no index.md.
    fs::write(tmp.join("docs/README.md"), "# Welcome\n\nstart here").unwrap();

    run_build(&tmp, LinkPolicy::Strict).unwrap();

    // `/` serves the README content...
    let index = fs::read_to_string(tmp.join("site/index.html")).unwrap();
    assert!(
        index.contains("start here"),
        "home should carry README body"
    );
    // ...and the README still resolves at its own url so links keep working.
    assert!(tmp.join("site/README.html").exists());
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_emits_linked_stylesheet_and_script() {
    let tmp =
        std::env::temp_dir().join(format!("compositor-build-assetlink-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("index.md"), "# Home\n\nhi").unwrap();

    run_build(&tmp, LinkPolicy::Strict).unwrap();

    // Default out_dir (see SiteConfig::out_dir) is "site".
    let out = tmp.join("site");
    let css = fs::read_to_string(out.join("assets/compositor.css")).unwrap();
    assert!(
        css.contains("Pico CSS"),
        "vendored Pico not concatenated in"
    );
    assert!(css.contains(".topbar"), "overrides not concatenated in");
    assert!(out.join("assets/compositor.js").is_file());

    let page = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(page.contains("assets/compositor.css"));
    assert!(
        !page.contains("<style>"),
        "CSS should be linked, not inlined"
    );
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_works_on_bare_markdown_dir_without_docs_subdir() {
    let tmp = std::env::temp_dir().join(format!("compositor-build-bare-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    // Markdown directly in the dir — no docs/ subdir, no compositor.toml.
    fs::write(tmp.join("index.md"), "# Home").unwrap();
    fs::write(tmp.join("logo.png"), b"PNG").unwrap();

    run_build(&tmp, LinkPolicy::Strict).unwrap();

    // Output lands in <dir>/site; the asset is mirrored.
    assert!(tmp.join("site/index.html").exists());
    assert!(tmp.join("site/logo.png").exists());
    // The copy_assets guard must stop the output being copied into itself.
    assert!(
        !tmp.join("site/site").exists(),
        "site/ must not be recursively copied into itself"
    );
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_stylesheet_carries_admonition_rules() {
    let tmp = std::env::temp_dir().join(format!("compositor-build-adm-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs")).unwrap();
    fs::write(tmp.join("compositor.toml"), "site_name = \"Test\"\n").unwrap();
    fs::write(tmp.join("docs/index.md"), "# Home\n").unwrap();

    run_build(&tmp, LinkPolicy::Strict).unwrap();

    let css = fs::read_to_string(tmp.join("site/assets/compositor.css")).unwrap();
    assert!(
        css.contains(".admonition"),
        "stylesheet missing admonition rules"
    );
    assert!(
        css.contains(".admonition.warning"),
        "stylesheet missing per-type accent"
    );
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_renders_admonition_into_html() {
    let tmp =
        std::env::temp_dir().join(format!("compositor-build-adm-html-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs")).unwrap();
    fs::write(tmp.join("compositor.toml"), "site_name = \"Test\"\n").unwrap();
    fs::write(
        tmp.join("docs/index.md"),
        "# Home\n\n!!! warning \"Heads up\"\n    Be **careful** here.\n",
    )
    .unwrap();

    run_build(&tmp, LinkPolicy::Strict).unwrap();

    let html = fs::read_to_string(tmp.join("site/index.html")).unwrap();
    assert!(
        html.contains("<div class=\"admonition warning\">"),
        "{html}"
    );
    assert!(
        html.contains("<p class=\"admonition-title\">Heads up</p>"),
        "{html}"
    );
    assert!(html.contains("<strong>careful</strong>"), "{html}");
    fs::remove_dir_all(&tmp).ok();
}
