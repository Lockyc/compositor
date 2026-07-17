use crate::config::SiteConfig;
use crate::render_page::render_page;
use anyhow::{anyhow, Context, Result};
use render_core::site::build_site;
use render_core::LinkPolicy;
use std::path::{Component, Path};
use walkdir::WalkDir;

pub fn run_build(project_dir: &Path, policy: LinkPolicy) -> Result<()> {
    let cfg = SiteConfig::load(project_dir)?;
    let docs = cfg.docs_path(project_dir);
    validate_out_dir(cfg.out_dir())?;
    let out = project_dir.join(cfg.out_dir());
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;

    let mut site = build_site(&docs, policy, &cfg.exclude)?;
    // A repo-root CLAUDE.md (outside the docs tree) is surfaced as a nav page.
    crate::render_page::surface_repo_claude(&mut site, &cfg, project_dir);
    // compositor owns the home page: a docs tree with no index.md still gets a
    // working `/` (see `resolve_home`).
    let home = crate::render_page::resolve_home(&site, &cfg, project_dir);
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
    copy_assets(&docs, &out, &cfg.exclude)?;
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
fn copy_assets(docs: &Path, out: &Path, exclude: &[String]) -> Result<()> {
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
        if render_core::exclude::is_excluded(rel, exclude) {
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
}
