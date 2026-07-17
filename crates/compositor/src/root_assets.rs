//! Image resolution for the two pages compositor renders from *outside* the docs
//! tree: a repo-root `README.md` promoted to the home page, and a repo-root
//! `CLAUDE.md` surfaced as a nav entry (see `render_page`).
//!
//! Those pages render with `page_dir = ""` — they sit at the site root — but their
//! image urls are relative to the **repo root**. Nothing else resolves against that
//! base, which is why this resolver exists rather than reusing `DocsAssets`.
//!
//! Two outcomes, both rewrites:
//!
//! - the file lands **inside** the docs dir → rewrite to its docs url. `copy_assets`
//!   already mirrored it; copying again would be a second source of the same bytes.
//! - the file sits **outside** the docs dir → record it for copy, mirroring its
//!   repo-relative path, so the url the author wrote resolves unchanged.
//!
//! Anything else (escapes the repo, missing, excluded) is `Keep` under a lenient
//! policy — an honest 404 — and a hard error under a strict `build`.

use anyhow::{anyhow, Result};
use render_core::markdown::{ImageResolution, ImageResolver};
use render_core::{Excluder, LinkPolicy};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

// Nothing outside `root_assets` constructs `RootAssets` yet — the next task
// wires it into `build`/`serve`, which will make every one of these allows
// dead weight to remove.
#[allow(dead_code)]
pub struct RootAssets<'a> {
    /// Canonical, so `strip_prefix` against `docs_dir` agrees.
    project_dir: PathBuf,
    docs_dir: PathBuf,
    excluder: &'a Excluder,
    policy: LinkPolicy,
    /// site url -> source file. `BTreeMap` for a deterministic copy order.
    copies: RefCell<BTreeMap<String, PathBuf>>,
}

impl<'a> RootAssets<'a> {
    #[allow(dead_code)]
    pub fn new(
        project_dir: &Path,
        docs_dir: &Path,
        excluder: &'a Excluder,
        policy: LinkPolicy,
    ) -> RootAssets<'a> {
        RootAssets {
            project_dir: canonical(project_dir),
            docs_dir: canonical(docs_dir),
            excluder,
            policy,
            copies: RefCell::new(BTreeMap::new()),
        }
    }

    /// Every outside-docs image actually referenced: site url -> source file.
    /// `build` copies exactly this set; `serve` serves exactly this set. Only
    /// resolved, non-excluded files are ever in it, so both are safe by
    /// construction.
    #[allow(dead_code)]
    pub fn copies(&self) -> BTreeMap<String, PathBuf> {
        self.copies.borrow().clone()
    }

    #[allow(dead_code)]
    fn unresolved(&self, url: &str) -> Result<ImageResolution> {
        match self.policy {
            LinkPolicy::Strict => Err(anyhow!("unresolvable image: {url} (from the repo root)")),
            // Lenient: emit the dead src rather than halt an unattended rebuild.
            LinkPolicy::Lenient => Ok(ImageResolution::Keep),
        }
    }
}

impl ImageResolver for RootAssets<'_> {
    fn resolve(&self, url: &str, _page_dir: &Path) -> Result<ImageResolution> {
        // `page_dir` is always "" here: a repo-root page sits at the site root,
        // so the base is the repo root itself.
        let Some(abs) = resolve_under(&self.project_dir, url) else {
            return self.unresolved(url);
        };
        // `resolve_under` only rules out `..` lexically, in the url string — it
        // never touches the filesystem. A symlink inside the repo pointing
        // outside it (`link -> /etc`) passes that check with `link/passwd`
        // looking contained, then `is_file` and both `strip_prefix` calls below
        // would follow the symlink and treat the target as repo content.
        // Canonicalizing resolves every symlink in the path, so re-checking
        // containment against it is what actually defends against the escape;
        // it also fails outright for a path that doesn't exist, so it folds in
        // the missing-file case too. From here on `abs` is canonical.
        let Ok(abs) = std::fs::canonicalize(&abs) else {
            return self.unresolved(url);
        };
        if !abs.starts_with(&self.project_dir) {
            return self.unresolved(url);
        }
        if !abs.is_file() {
            return self.unresolved(url);
        }
        if let Ok(docs_rel) = abs.strip_prefix(&self.docs_dir) {
            if self.excluder.is_excluded(docs_rel) {
                return self.unresolved(url);
            }
            // Already mirrored by `copy_assets` — rewrite, don't copy.
            return Ok(ImageResolution::Rewrite(to_url(docs_rel)));
        }
        if self.excluder.is_gitignored(&abs) {
            return self.unresolved(url);
        }
        let Ok(rel) = abs.strip_prefix(&self.project_dir) else {
            return self.unresolved(url);
        };
        let site_url = to_url(rel);
        self.copies.borrow_mut().insert(site_url.clone(), abs);
        Ok(ImageResolution::Rewrite(site_url))
    }
}

#[allow(dead_code)]
fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[allow(dead_code)]
fn to_url(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Join `rel` onto `base`, returning `None` if it escapes `base` or is absolute.
///
/// Deliberately **not** render-core's `normalize`: that pops a `..` even when the
/// accumulated path is empty, so `../../etc/passwd` normalizes to `etc/passwd` —
/// an escape silently rewritten into an innocent-looking relative path. Escape
/// detection has to fail, not normalize.
#[allow(dead_code)]
fn resolve_under(base: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in Path::new(rel).components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            Component::Normal(x) => out.push(x),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(base.join(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("compositor-root-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("docs")).unwrap();
        fs::create_dir_all(d.join(".git")).unwrap();
        d
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    #[test]
    fn outside_docs_image_is_recorded_and_keeps_its_url() {
        let d = repo("outside");
        write(&d, "images/logo.png", "x");
        let ex = Excluder::new(&d, &d.join("docs"), &[]);
        let r = RootAssets::new(&d, &d.join("docs"), &ex, LinkPolicy::Strict);

        let got = r.resolve("images/logo.png", Path::new("")).unwrap();
        // Mirrored repo-relative, so the url the README wrote still resolves.
        assert!(
            matches!(&got, ImageResolution::Rewrite(u) if u == "images/logo.png"),
            "got: {got:?}"
        );
        assert_eq!(r.copies().len(), 1, "the file must be recorded for copy");
        assert!(r.copies().contains_key("images/logo.png"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn into_docs_image_is_rewritten_to_its_docs_url_and_not_copied() {
        // copy_assets already mirrored it; a second copy would be a second
        // source of the same bytes.
        let d = repo("into-docs");
        write(&d, "docs/images/logo.png", "x");
        let ex = Excluder::new(&d, &d.join("docs"), &[]);
        let r = RootAssets::new(&d, &d.join("docs"), &ex, LinkPolicy::Strict);

        let got = r.resolve("docs/images/logo.png", Path::new("")).unwrap();
        assert!(
            matches!(&got, ImageResolution::Rewrite(u) if u == "images/logo.png"),
            "got: {got:?}"
        );
        assert!(
            r.copies().is_empty(),
            "an in-docs asset must not be copied twice"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn gitignored_outside_docs_image_is_never_recorded() {
        let d = repo("gitignored");
        write(&d, ".gitignore", "scratch/\n");
        write(&d, "scratch/secret.png", "x");
        let ex = Excluder::new(&d, &d.join("docs"), &[]);
        let r = RootAssets::new(&d, &d.join("docs"), &ex, LinkPolicy::Lenient);

        let got = r.resolve("scratch/secret.png", Path::new("")).unwrap();
        assert!(matches!(got, ImageResolution::Keep), "got: {got:?}");
        assert!(
            r.copies().is_empty(),
            "a gitignored file must never be copied"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn excluded_in_docs_image_is_never_resolved() {
        let d = repo("excluded");
        write(&d, "docs/private/logo.png", "x");
        let ex = Excluder::new(&d, &d.join("docs"), &["private/".to_string()]);
        let r = RootAssets::new(&d, &d.join("docs"), &ex, LinkPolicy::Lenient);

        let got = r.resolve("docs/private/logo.png", Path::new("")).unwrap();
        assert!(matches!(got, ImageResolution::Keep), "got: {got:?}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn image_escaping_the_repo_is_refused() {
        // `normalize` pops `..` even from an empty base, so escape detection
        // cannot lean on it — this is what `resolve_under` guards.
        let d = repo("escape");
        let ex = Excluder::new(&d, &d.join("docs"), &[]);
        let r = RootAssets::new(&d, &d.join("docs"), &ex, LinkPolicy::Lenient);

        let got = r.resolve("../../etc/passwd", Path::new("")).unwrap();
        assert!(matches!(got, ImageResolution::Keep), "got: {got:?}");
        assert!(
            r.copies().is_empty(),
            "a path outside the repo must never be copied"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn image_reached_through_a_symlink_out_of_the_repo_is_refused() {
        // `resolve_under` is lexical: `link/passwd` has no `..`, so the url alone
        // looks contained. Only the resolved path reveals the escape — and this
        // resolver's output is copied by `build` and served by `serve`.
        let d = repo("symlink-escape");
        let outside =
            std::env::temp_dir().join(format!("compositor-outside-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.png"), "SECRET").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();

        let ex = Excluder::new(&d, &d.join("docs"), &[]);
        let r = RootAssets::new(&d, &d.join("docs"), &ex, LinkPolicy::Lenient);

        let got = r.resolve("link/secret.png", Path::new("")).unwrap();
        assert!(matches!(got, ImageResolution::Keep), "got: {got:?}");
        assert!(
            r.copies().is_empty(),
            "a file reached through a symlink out of the repo must never be copied"
        );
        let _ = fs::remove_dir_all(&d);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn image_reached_through_a_symlink_inside_the_repo_still_resolves() {
        // The escape guard canonicalizes before trusting the filesystem; a
        // symlink whose target is legitimately inside the repo must keep
        // resolving, not just get swept up by the containment check.
        let d = repo("symlink-inside");
        write(&d, "docs/images/logo.png", "x");
        std::os::unix::fs::symlink(d.join("docs/images/logo.png"), d.join("link.png")).unwrap();
        let ex = Excluder::new(&d, &d.join("docs"), &[]);
        let r = RootAssets::new(&d, &d.join("docs"), &ex, LinkPolicy::Lenient);

        let got = r.resolve("link.png", Path::new("")).unwrap();
        assert!(
            matches!(&got, ImageResolution::Rewrite(u) if u == "images/logo.png"),
            "got: {got:?}"
        );
        assert!(
            r.copies().is_empty(),
            "an in-docs asset reached via a symlink must not be copied twice"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_image_errors_under_strict_and_degrades_under_lenient() {
        let d = repo("missing");
        let ex = Excluder::new(&d, &d.join("docs"), &[]);

        let strict = RootAssets::new(&d, &d.join("docs"), &ex, LinkPolicy::Strict);
        let err = strict
            .resolve("images/gone.png", Path::new(""))
            .unwrap_err();
        assert!(err.to_string().contains("images/gone.png"), "got: {err}");

        let lenient = RootAssets::new(&d, &d.join("docs"), &ex, LinkPolicy::Lenient);
        let got = lenient.resolve("images/gone.png", Path::new("")).unwrap();
        assert!(matches!(got, ImageResolution::Keep), "got: {got:?}");
        let _ = fs::remove_dir_all(&d);
    }
}
