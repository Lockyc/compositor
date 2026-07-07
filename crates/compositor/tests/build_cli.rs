use compositor::build::run_build;
use std::fs;

#[test]
fn build_writes_html_files() {
    let tmp = std::env::temp_dir().join(format!("compositor-build-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("docs/cli")).unwrap();
    fs::write(tmp.join("compositor.toml"), "site_name = \"Test\"\n").unwrap();
    fs::write(tmp.join("docs/index.md"), "# Home\n\n[tar](cli/tar.md)").unwrap();
    fs::write(tmp.join("docs/cli/tar.md"), "# Tar\n\nbody").unwrap();

    run_build(&tmp).unwrap();

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

    run_build(&tmp).unwrap();

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

    let result = run_build(&tmp);

    assert!(result.is_err(), "run_build must reject out_dir = \".\"");
    assert!(
        tmp.join("docs/index.md").exists(),
        "the project's docs/ must survive a rejected build, not be deleted"
    );
    fs::remove_dir_all(&tmp).ok();
}
