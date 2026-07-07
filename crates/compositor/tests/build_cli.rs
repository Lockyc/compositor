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
