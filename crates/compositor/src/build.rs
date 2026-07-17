use crate::config::SiteConfig;
use crate::render_page::render_page;
use anyhow::{anyhow, Context, Result};
use render_core::site::build_site;
use render_core::{Excluder, LinkPolicy};
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

pub fn run_build(project_dir: &Path, policy: LinkPolicy) -> Result<()> {
    let cfg = SiteConfig::load(project_dir)?;
    let docs = cfg.docs_path(project_dir);
    validate_out_dir(cfg.out_dir())?;
    let out = project_dir.join(cfg.out_dir());
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;

    let excluder = Excluder::new(project_dir, &docs, &cfg.exclude);
    for w in excluder.warnings() {
        eprintln!("warning: {w}");
    }

    let mut site = build_site(&docs, policy, &excluder)?;
    // The two pages compositor renders from outside the docs tree resolve their
    // images against the repo root; `images` records what must be copied.
    let images = crate::root_assets::RootAssets::new(project_dir, &docs, &excluder, policy);
    // A repo-root CLAUDE.md (outside the docs tree) is surfaced as a nav page.
    crate::render_page::surface_repo_claude(&mut site, &cfg, project_dir, &images)?;
    // compositor owns the home page: a docs tree with no index.md still gets a
    // working `/` (see `resolve_home`).
    let home = crate::render_page::resolve_home(&site, &cfg, project_dir, &images)?;
    let order = crate::render_page::reading_order(&site.nav, home.as_ref());
    for page in site.pages.iter().chain(home.as_ref()) {
        let (prev, next) = crate::render_page::neighbours(&order, &page.url);
        let html = render_page(&cfg, &site.nav, page, prev, next);
        let dest = out.join(&page.url);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, html)?;
    }
    copy_assets(&docs, &out, &excluder)?;
    copy_root_assets(&out, &images.copies())?;
    write_shell_assets(&out)?;

    println!("built {} pages -> {}", site.pages.len(), out.display());
    Ok(())
}

/// Guard against an `out_dir` that would make `remove_dir_all(out)` delete the
/// project directory itself (or an ancestor of it) — e.g. a misconfigured
/// `out_dir = "."`, `".."`, or `""`.
fn validate_out_dir(out_dir: &str) -> Result<()> {
    if out_dir.trim().is_empty() {
        return Err(anyhow!("out_dir must not be empty"));
    }
    let path = Path::new(out_dir);
    let mut components = path.components().peekable();
    if components.peek().is_none() {
        return Err(anyhow!("out_dir {out_dir:?} is invalid"));
    }
    for component in components {
        match component {
            // "." alone (or repeated, e.g. "./.") never descends anywhere,
            // so it normalizes to the project dir itself.
            Component::CurDir => {
                return Err(anyhow!("out_dir must not be \".\""));
            }
            // Any ".." component can walk back out of the project dir and
            // into an ancestor.
            Component::ParentDir => {
                return Err(anyhow!(
                    "out_dir {out_dir:?} must not contain a \"..\" component"
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Mirror every non-Markdown file in `docs` into `out`, preserving relative
/// paths. Markdown is rendered to HTML elsewhere; everything else (images,
/// downloads, data files a page links to) is copied verbatim so those
/// references resolve in the built site — matching MkDocs, which copies all
/// non-doc files from the docs dir into the output.
fn copy_assets(docs: &Path, out: &Path, excluder: &Excluder) -> Result<()> {
    // Skip the output tree: when the docs dir *is* the project dir (a bare
    // Markdown folder with no config), `out` sits inside `docs`, so without
    // this the walk would copy the freshly-written site back into itself.
    for entry in WalkDir::new(docs)
        .into_iter()
        .filter_entry(|e| !e.path().starts_with(out))
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            continue;
        }
        let rel = path.strip_prefix(docs)?;
        if excluder.is_excluded(rel) {
            continue;
        }
        let dest = out.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(path, &dest).with_context(|| format!("copying asset {}", path.display()))?;
    }
    Ok(())
}

/// Copy the outside-docs assets the repo-root pages actually referenced,
/// mirroring each one's repo-relative path so the url the author wrote resolves.
///
/// Runs *after* `copy_assets`, and skips a destination that already exists: the
/// docs tree is the primary source, so docs content wins a url collision.
/// `write_shell_assets` runs after both and wins over either, as before.
fn copy_root_assets(
    out: &Path,
    copies: &std::collections::BTreeMap<String, PathBuf>,
) -> Result<()> {
    for (url, src) in copies {
        let dest = out.join(url);
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, &dest)
            .with_context(|| format!("copying root asset {}", src.display()))?;
    }
    Ok(())
}

/// Emit the embedded shell assets (Pico + overrides stylesheet, and the JS) into
/// `out/assets/`. Written after `copy_assets` so a docs file of the same name
/// can't clobber them.
fn write_shell_assets(out: &Path) -> Result<()> {
    let dir = out.join("assets");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        out.join(crate::assets::CSS_URL),
        crate::assets::stylesheet(),
    )?;
    std::fs::write(
        out.join(crate::assets::JS_URL),
        crate::assets::COMPOSITOR_JS,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use render_core::LinkPolicy;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("compositor-build-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("docs")).unwrap();
        p
    }

    #[test]
    fn broken_link_fails_strict_but_survives_lenient() {
        let tmp = scratch("policy");
        // A page whose internal link resolves to nothing.
        std::fs::write(tmp.join("docs/a.md"), "# A\n\n[dead](missing.md)\n").unwrap();

        let strict = run_build(&tmp, LinkPolicy::Strict);
        assert!(
            strict.is_err(),
            "strict build should reject the broken link"
        );

        let lenient = run_build(&tmp, LinkPolicy::Lenient);
        assert!(
            lenient.is_ok(),
            "lenient build must publish anyway: {lenient:?}"
        );
        assert!(
            tmp.join("site/a.html").exists(),
            "lenient build wrote the page"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn excluded_dir_is_not_rendered_or_copied() {
        let tmp = scratch("exclude");
        std::fs::create_dir_all(tmp.join("docs/superpowers")).unwrap();
        std::fs::write(tmp.join("docs/keep.md"), "# Keep\n").unwrap();
        std::fs::write(tmp.join("docs/superpowers/secret.md"), "# Secret\n").unwrap();
        std::fs::write(tmp.join("docs/superpowers/note.txt"), "raw asset").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\nexclude = [\"superpowers/\"]\n",
        )
        .unwrap();

        run_build(&tmp, LinkPolicy::Lenient).unwrap();

        assert!(tmp.join("site/keep.html").exists(), "kept page rendered");
        assert!(
            !tmp.join("site/superpowers/secret.html").exists(),
            "excluded md not rendered"
        );
        assert!(
            !tmp.join("site/superpowers/note.txt").exists(),
            "excluded asset not copied"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn gitignored_dir_is_not_rendered_or_copied() {
        let tmp = scratch("gitignore");
        std::fs::create_dir_all(tmp.join(".git")).unwrap();
        std::fs::write(tmp.join(".gitignore"), "docs/superpowers/\n").unwrap();
        std::fs::create_dir_all(tmp.join("docs/superpowers")).unwrap();
        std::fs::write(tmp.join("docs/index.md"), "# Home\n").unwrap();
        std::fs::write(tmp.join("docs/superpowers/spec.md"), "# Spec\n").unwrap();
        std::fs::write(tmp.join("docs/superpowers/note.txt"), "scratch").unwrap();

        // No compositor.toml at all: gitignore alone must do the work.
        run_build(&tmp, LinkPolicy::Strict).unwrap();

        assert!(
            !tmp.join("site/superpowers/spec.html").exists(),
            "gitignored md must not be rendered"
        );
        assert!(
            !tmp.join("site/superpowers/note.txt").exists(),
            "gitignored asset must not be copied"
        );
        assert!(
            tmp.join("site/index.html").exists(),
            "kept page must render"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn repo_root_readme_image_outside_docs_is_copied_and_resolves() {
        // The reported bug: the home page is promoted from a repo-root README whose
        // image sits outside the docs tree, so nothing ever copied it.
        let tmp = scratch("root-img");
        std::fs::create_dir_all(tmp.join("images")).unwrap();
        std::fs::write(tmp.join("images/logo.png"), "PNG").unwrap();
        std::fs::write(tmp.join("README.md"), "# P\n\n![logo](images/logo.png)\n").unwrap();
        std::fs::write(tmp.join("docs/guide.md"), "# Guide\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\n",
        )
        .unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();

        let home = std::fs::read_to_string(tmp.join("site/index.html")).unwrap();
        assert!(home.contains(r#"src="images/logo.png""#), "home: {home}");
        assert!(
            tmp.join("site/images/logo.png").exists(),
            "the referenced root asset must be copied into the site"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn repo_root_readme_image_inside_docs_is_rewritten_not_duplicated() {
        let tmp = scratch("root-img-indocs");
        std::fs::create_dir_all(tmp.join("docs/images")).unwrap();
        std::fs::write(tmp.join("docs/images/logo.png"), "PNG").unwrap();
        std::fs::write(
            tmp.join("README.md"),
            "# P\n\n![logo](docs/images/logo.png)\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\n",
        )
        .unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();

        let home = std::fs::read_to_string(tmp.join("site/index.html")).unwrap();
        assert!(home.contains(r#"src="images/logo.png""#), "home: {home}");
        assert!(tmp.join("site/images/logo.png").exists());
        assert!(
            !tmp.join("site/docs/images/logo.png").exists(),
            "an in-docs asset must not be copied a second time under its repo path"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn repo_root_claude_image_is_copied_too() {
        let tmp = scratch("root-img-claude");
        std::fs::create_dir_all(tmp.join("images")).unwrap();
        std::fs::write(tmp.join("images/d.png"), "PNG").unwrap();
        std::fs::write(tmp.join("CLAUDE.md"), "# Notes\n\n![d](images/d.png)\n").unwrap();
        std::fs::write(tmp.join("docs/index.md"), "# Home\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\n",
        )
        .unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();

        let page = std::fs::read_to_string(tmp.join("site/CLAUDE.html")).unwrap();
        assert!(page.contains(r#"src="images/d.png""#), "page: {page}");
        assert!(tmp.join("site/images/d.png").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn gitignored_root_image_is_never_copied() {
        let tmp = scratch("root-img-gi");
        std::fs::create_dir_all(tmp.join(".git")).unwrap();
        std::fs::create_dir_all(tmp.join("scratch")).unwrap();
        std::fs::write(tmp.join(".gitignore"), "scratch/\n").unwrap();
        std::fs::write(tmp.join("scratch/secret.png"), "PNG").unwrap();
        std::fs::write(tmp.join("README.md"), "# P\n\n![s](scratch/secret.png)\n").unwrap();
        std::fs::write(tmp.join("docs/guide.md"), "# Guide\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\n",
        )
        .unwrap();

        // Lenient: a gitignored image is "unresolvable", which strict would reject.
        run_build(&tmp, LinkPolicy::Lenient).unwrap();

        assert!(
            !tmp.join("site/scratch/secret.png").exists(),
            "a gitignored file must never reach the site"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn dead_root_readme_image_fails_strict_but_survives_lenient() {
        let tmp = scratch("root-img-dead");
        std::fs::write(tmp.join("README.md"), "# P\n\n![gone](images/gone.png)\n").unwrap();
        std::fs::write(tmp.join("docs/guide.md"), "# Guide\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\n",
        )
        .unwrap();

        let strict = run_build(&tmp, LinkPolicy::Strict);
        assert!(
            strict.is_err(),
            "a dead README image must fail a strict build"
        );
        assert!(
            strict.unwrap_err().to_string().contains("images/gone.png"),
            "the error must name the image"
        );

        run_build(&tmp, LinkPolicy::Lenient).unwrap();
        let home = std::fs::read_to_string(tmp.join("site/index.html")).unwrap();
        assert!(home.contains(r#"src="images/gone.png""#), "home: {home}");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn readme_badge_urls_survive_a_strict_build() {
        // compositor's own README is badge-only; absolute urls are not ours to resolve.
        let tmp = scratch("root-img-badge");
        std::fs::write(
            tmp.join("README.md"),
            "# P\n\n![CI](https://example.com/b.svg)\n",
        )
        .unwrap();
        std::fs::write(tmp.join("docs/guide.md"), "# Guide\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\n",
        )
        .unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();
        let home = std::fs::read_to_string(tmp.join("site/index.html")).unwrap();
        assert!(
            home.contains(r#"src="https://example.com/b.svg""#),
            "home: {home}"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn docs_asset_wins_a_url_collision_with_a_repo_root_asset() {
        // Global constraint (see `copy_root_assets`'s doc comment): docs content
        // always wins when a repo-root README/CLAUDE image and a real docs asset
        // land on the same site url. Nothing else guards this — `copy_root_assets`
        // running after `copy_assets` and skipping an existing destination is the
        // whole mechanism, and a future refactor could silently invert it.
        let tmp = scratch("root-img-collision");
        std::fs::create_dir_all(tmp.join("images")).unwrap();
        std::fs::write(tmp.join("images/logo.png"), "OUTSIDE").unwrap();
        std::fs::create_dir_all(tmp.join("docs/images")).unwrap();
        std::fs::write(tmp.join("docs/images/logo.png"), "DOCS").unwrap();
        std::fs::write(tmp.join("README.md"), "# P\n\n![logo](images/logo.png)\n").unwrap();
        std::fs::write(tmp.join("docs/guide.md"), "# Guide\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\n",
        )
        .unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();

        let got = std::fs::read_to_string(tmp.join("site/images/logo.png")).unwrap();
        assert_eq!(got, "DOCS", "docs content must win the url collision");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
