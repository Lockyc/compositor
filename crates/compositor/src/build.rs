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
    let out = resolve_out_dir(project_dir, &docs, cfg.out_dir())?;

    let excluder = Excluder::new(project_dir, &docs, &cfg.exclude);
    for w in excluder.warnings() {
        eprintln!("warning: {w}");
    }

    // Every content error (a strict-policy dead link or image) surfaces here, before
    // anything is written; an IO error later fails in staging (`swap_into_place`).
    // Either way a failed build leaves the last good site in place.
    let mut site = build_site(&docs, policy, &excluder, false)?;
    // The two pages compositor renders from outside the docs tree resolve their
    // images against the repo root; `images` records what must be copied.
    let images = crate::root_assets::RootAssets::new(project_dir, &docs, &excluder, policy);
    // Repo-root CLAUDE.md / AGENTS.md (outside the docs tree) surfaced as nav pages.
    crate::render_page::surface_repo_agent_files(&mut site, &cfg, project_dir, &images, false)?;
    // compositor owns the home page: a docs tree with no index.md still gets a
    // working `/` (see `resolve_home`). `build` ships no edit scaffolding.
    let home = crate::render_page::resolve_home(&site, &cfg, project_dir, &images, false)?;
    let order = crate::render_page::reading_order(&site.nav, home.as_ref());

    ensure_build_output(&out)?;
    let staging = sibling(&out, &format!("tmp-{}", std::process::id()));
    remove_if_present(&staging)?;
    let written = (|| -> Result<()> {
        std::fs::create_dir_all(&staging)
            .with_context(|| format!("creating {}", staging.display()))?;
        for page in site.pages.iter().chain(home.as_ref()) {
            let (prev, next) = crate::render_page::neighbours(&order, &page.url);
            let html = render_page(&cfg, &site.nav, page, prev, next);
            let dest = staging.join(&page.url);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, html)?;
        }
        copy_assets(&docs, &out, &staging, &excluder)?;
        copy_root_assets(&staging, &images.copies())?;
        write_shell_assets(&staging)?;
        swap_into_place(&staging, &out)
    })();
    if written.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    written?;

    println!("built {} pages -> {}", site.pages.len(), out.display());
    Ok(())
}

/// `<out>.<suffix>`, beside `out` — the same filesystem, so a rename swaps it.
fn sibling(out: &Path, suffix: &str) -> PathBuf {
    let mut name = out.file_name().unwrap_or_default().to_owned();
    name.push(format!(".{suffix}"));
    out.with_file_name(name)
}

/// True for `out` and for any `<out>.tmp-*` build scratch beside it, including one
/// a killed build left behind. In a bare folder `out` sits inside the docs dir, so
/// the asset walk must skip all of them or it copies a site into itself.
fn is_build_output(path: &Path, out: &Path) -> bool {
    if path.starts_with(out) {
        return true;
    }
    let (Some(parent), Some(stem)) = (out.parent(), out.file_name()) else {
        return false;
    };
    let mut prefix = stem.to_owned();
    prefix.push(".tmp-");
    let prefix = prefix.to_string_lossy().into_owned();
    path.strip_prefix(parent)
        .ok()
        .and_then(|rel| rel.components().next())
        .and_then(|first| first.as_os_str().to_str())
        .is_some_and(|first| first.starts_with(&prefix))
}

fn remove_if_present(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("removing {}", dir.display()))
        }
        _ => Ok(()),
    }
}

/// Replace `out` with the fully-written `staging` by two renames, so `out` is only
/// ever the previous site or the new one, never a partial one. The previous site
/// is moved aside before it is deleted: a delete that fails partway then leaves
/// debris beside `out`, never stale pages inside it.
fn swap_into_place(staging: &Path, out: &Path) -> Result<()> {
    let old = sibling(staging, "old");
    remove_if_present(&old)?;
    let had_old = match std::fs::rename(out, &old) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e).with_context(|| format!("moving aside {}", out.display())),
    };
    if let Err(e) = std::fs::rename(staging, out) {
        if had_old {
            let _ = std::fs::rename(&old, out);
        }
        return Err(e).with_context(|| format!("moving the new build into {}", out.display()));
    }
    if had_old {
        if let Err(e) = std::fs::remove_dir_all(&old) {
            eprintln!(
                "warning: could not remove the previous build {}: {e}",
                old.display()
            );
        }
    }
    Ok(())
}

/// Refuse to wipe an `out` that holds something other than a compositor build. It
/// must be absent, empty, or carry the stylesheet every build writes — otherwise a
/// bare notes folder with its own `site/` (the no-config default out_dir) would be
/// deleted outright on the first build.
fn ensure_build_output(out: &Path) -> Result<()> {
    let empty = match std::fs::read_dir(out) {
        Ok(mut entries) => entries.next().is_none(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", out.display())),
    };
    if empty || out.join(crate::assets::CSS_URL).is_file() {
        return Ok(());
    }
    Err(anyhow!(
        "{} exists and is not a compositor build (no {}); refusing to delete it — set out_dir to another directory",
        out.display(),
        crate::assets::CSS_URL
    ))
}

/// Resolve `out_dir` against `project_dir`, refusing any value that would make
/// `remove_dir_all(out)` delete sources: the project directory, the docs
/// directory, or an ancestor of either. That covers `"."`, `".."`, `""`, an
/// absolute path such as `"/"` (an absolute `out_dir` replaces the join
/// outright), and `out_dir == docs_dir`. An absolute `out_dir` outside the
/// project (`/var/www/site`) is legitimate and resolves to itself.
fn resolve_out_dir(project_dir: &Path, docs: &Path, out_dir: &str) -> Result<PathBuf> {
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
    let out = project_dir.join(out_dir);
    let out_canon = canonical_lenient(&out);
    for (what, protected) in [("project directory", project_dir), ("docs directory", docs)] {
        if canonical_lenient(protected).starts_with(&out_canon) {
            return Err(anyhow!(
                "out_dir {out_dir:?} would delete the {what} {} on build; point it elsewhere",
                protected.display()
            ));
        }
    }
    Ok(out)
}

/// `path` canonicalized through its deepest existing ancestor, so a not-yet-
/// created `out_dir` still compares against canonical roots (macOS `/tmp` is
/// `/private/tmp`). Components below that ancestor are appended as written;
/// `resolve_out_dir` has already refused `.`/`..`, so they are plain names.
fn canonical_lenient(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(c) = std::fs::canonicalize(cur) {
            return tail.iter().rev().fold(c, |acc: PathBuf, n| acc.join(n));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_owned());
                cur = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Mirror every non-Markdown file in `docs` into `dest_root` (the staging dir of
/// the build whose final home is `out`), preserving relative paths. Markdown is
/// rendered to HTML elsewhere; everything else (images, downloads, data files a
/// page links to) is copied verbatim so those references resolve in the built
/// site — matching MkDocs, which copies all non-doc files from the docs dir into
/// the output.
fn copy_assets(docs: &Path, out: &Path, dest_root: &Path, excluder: &Excluder) -> Result<()> {
    // Skip the output tree and its staging siblings (`is_build_output`).
    // Excluded directories are pruned rather than walked file by file.
    for entry in WalkDir::new(docs).into_iter().filter_entry(|e| {
        !is_build_output(e.path(), out)
            && (e.depth() == 0
                || !e.file_type().is_dir()
                || e.path()
                    .strip_prefix(docs)
                    .is_ok_and(|rel| !excluder.is_excluded_dir(rel)))
    }) {
        let entry = entry?;
        let path = entry.path();
        if !excluder.is_contained_file(path) {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            continue;
        }
        let rel = path.strip_prefix(docs)?;
        if excluder.is_excluded(rel) {
            continue;
        }
        let dest = dest_root.join(rel);
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

    #[test]
    fn docs_image_with_percent_encoded_space_resolves_under_strict() {
        // Regression: an author must percent-encode a space for Markdown to parse
        // the url; the asset on disk ("my image.png") is decoded, so a strict
        // build must not treat the encoded reference as unresolvable.
        let tmp = scratch("img-encoded");
        std::fs::write(tmp.join("docs/my image.png"), "PNG").unwrap();
        std::fs::write(
            tmp.join("docs/index.md"),
            "# Home\n\n![spaces](my%20image.png)\n",
        )
        .unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();

        let home = std::fs::read_to_string(tmp.join("site/index.html")).unwrap();
        assert!(
            home.contains(r#"src="my%20image.png""#),
            "the emitted src must stay exactly as written: {home}"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn docs_image_with_fragment_resolves_under_strict() {
        let tmp = scratch("img-fragment");
        std::fs::write(tmp.join("docs/sprite.svg"), "<svg></svg>").unwrap();
        std::fs::write(
            tmp.join("docs/index.md"),
            "# Home\n\n![sprite](sprite.svg#icon)\n",
        )
        .unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();

        let home = std::fs::read_to_string(tmp.join("site/index.html")).unwrap();
        assert!(
            home.contains(r#"src="sprite.svg#icon""#),
            "the emitted src must stay exactly as written: {home}"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
    #[test]
    fn out_dir_equal_to_docs_dir_is_refused_and_sources_survive() {
        // A build replaces `out` wholesale, so `out_dir == docs_dir` would
        // discard every source file.
        let tmp = scratch("out-eq-docs");
        std::fs::write(tmp.join("docs/index.md"), "# Home\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\ndocs_dir = \"docs\"\nout_dir = \"docs\"\n",
        )
        .unwrap();

        let err = run_build(&tmp, LinkPolicy::Lenient).expect_err("out_dir == docs_dir");
        assert!(err.to_string().contains("out_dir"), "named error: {err}");
        assert!(tmp.join("docs/index.md").exists(), "sources must survive");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn absolute_out_dir_at_the_project_is_refused_and_sources_survive() {
        // An absolute `out_dir` replaces the project-relative join outright, so
        // naming the project (or any ancestor) would delete it.
        let tmp = scratch("out-abs-project");
        std::fs::write(tmp.join("docs/index.md"), "# Home\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            format!(
                "site_name = \"X\"\nout_dir = {:?}\n",
                tmp.display().to_string()
            ),
        )
        .unwrap();

        let err = run_build(&tmp, LinkPolicy::Lenient).expect_err("out_dir == project");
        assert!(err.to_string().contains("out_dir"), "named error: {err}");
        assert!(tmp.join("docs/index.md").exists(), "sources must survive");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn resolve_out_dir_refuses_the_filesystem_root() {
        // Asserted on the resolver, never through `run_build`: a regression
        // here must fail a test, not delete `/`.
        let tmp = scratch("out-root");
        let docs = tmp.join("docs");
        assert!(resolve_out_dir(&tmp, &docs, "/").is_err());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn absolute_out_dir_outside_the_project_still_builds() {
        let tmp = scratch("out-abs-ok");
        let elsewhere = std::env::temp_dir().join(format!(
            "compositor-build-out-abs-ok-dest-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&elsewhere);
        std::fs::write(tmp.join("docs/index.md"), "# Home\n").unwrap();
        std::fs::write(
            tmp.join("compositor.toml"),
            format!(
                "site_name = \"X\"\nout_dir = {:?}\n",
                elsewhere.display().to_string()
            ),
        )
        .unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();
        assert!(elsewhere.join("index.html").exists());
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::remove_dir_all(&elsewhere).ok();
    }

    #[cfg(unix)]
    #[test]
    fn io_failure_mid_build_leaves_the_previous_site_intact() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = scratch("io-fail");
        std::fs::write(tmp.join("docs/index.md"), "# Home\n").unwrap();
        std::fs::write(tmp.join("docs/old.md"), "# Old\n").unwrap();
        run_build(&tmp, LinkPolicy::Strict).unwrap();

        // Pages render, then the asset copy hits a file it cannot read.
        let locked = tmp.join("docs/locked.png");
        std::fs::write(&locked, "PNG").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::write(tmp.join("docs/index.md"), "# Changed\n").unwrap();
        let result = run_build(&tmp, LinkPolicy::Strict);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(result.is_err(), "an unreadable asset must fail the build");
        let home = std::fs::read_to_string(tmp.join("site/index.html")).unwrap();
        assert!(home.contains("Home"), "previous home survives: {home}");
        assert!(tmp.join("site/old.html").exists(), "previous pages survive");
        assert!(tmp.join("site").join(crate::assets::CSS_URL).is_file());
        let leftovers: Vec<_> = std::fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("site."))
            .collect();
        assert!(leftovers.is_empty(), "staging cleaned up: {leftovers:?}");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn rebuild_drops_a_deleted_page() {
        let tmp = scratch("rebuild-drop");
        std::fs::write(tmp.join("docs/index.md"), "# Home\n").unwrap();
        std::fs::write(tmp.join("docs/gone.md"), "# Gone\n").unwrap();
        run_build(&tmp, LinkPolicy::Strict).unwrap();
        assert!(tmp.join("site/gone.html").exists());

        std::fs::remove_file(tmp.join("docs/gone.md")).unwrap();
        run_build(&tmp, LinkPolicy::Strict).unwrap();

        assert!(tmp.join("site/index.html").exists());
        assert!(
            !tmp.join("site/gone.html").exists(),
            "a deleted page must not survive a rebuild"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn bare_folder_build_skips_leftover_staging_dirs() {
        // A killed build leaves `site.tmp-<pid>` inside a bare folder's docs dir.
        let tmp = std::env::temp_dir().join(format!(
            "compositor-build-bare-stage-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("site.tmp-1")).unwrap();
        std::fs::write(tmp.join("site.tmp-1/debris.png"), "PNG").unwrap();
        std::fs::write(tmp.join("index.md"), "# Home\n").unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();
        run_build(&tmp, LinkPolicy::Strict).unwrap();

        assert!(tmp.join("site/index.html").exists());
        assert!(
            !tmp.join("site/site.tmp-1").exists(),
            "staging debris not copied"
        );
        assert!(
            !tmp.join("site/site").exists(),
            "previous site not copied in"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn bare_folder_build_never_publishes_the_git_dir() {
        // No `docs/` subdir: the docs dir is the repo root, so the asset walk
        // passes straight over `.git/`.
        let tmp =
            std::env::temp_dir().join(format!("compositor-build-bare-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".git/refs")).unwrap();
        std::fs::write(tmp.join(".git/config"), "[remote]\n").unwrap();
        std::fs::write(tmp.join(".git/refs/HEAD"), "ref").unwrap();
        std::fs::write(tmp.join("index.md"), "# Home\n").unwrap();
        std::fs::write(tmp.join("logo.png"), "PNG").unwrap();

        run_build(&tmp, LinkPolicy::Strict).unwrap();

        assert!(tmp.join("site/logo.png").exists(), "ordinary asset copied");
        assert!(
            !tmp.join("site/.git").exists(),
            "the git database must never reach the site"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
}
