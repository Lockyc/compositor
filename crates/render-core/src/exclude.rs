use ignore::gitignore::Gitignore;
use ignore::Match;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Decides whether a docs-tree path is kept out of the site. Two independent
/// rules, unioned:
///
/// - `compositor.toml`'s `exclude` — docs-dir-relative path prefixes, for a
///   *tracked* tree that is kept in git but deliberately not published.
/// - the repo's `.gitignore` files — for *untracked* scratch, which is not site
///   content by definition.
///
/// Built once per build/rebuild and shared by every exclusion point (rendering,
/// asset-copy, `serve`'s on-demand assets), so the three cannot disagree.
///
/// **Repo `.gitignore` files only** — never the global `~/.config/git/ignore` or
/// `.git/info/exclude`. Those are machine-local, and honoring them would make
/// the same repo render differently on a laptop than on a build host. Every
/// input to a render stays in the tree.
pub struct Excluder {
    patterns: Vec<String>,
    /// Canonical, so `docs_dir.join(rel)` is anchored the way `.gitignore`
    /// patterns are. Falls back to the path as given if canonicalization fails.
    docs_dir: PathBuf,
    /// Canonical repo root, or `None` outside a git repo. Bounds the ancestor
    /// walk for a path with no docs-relative form (see `is_gitignored`).
    repo_root: Option<PathBuf>,
    /// One matcher per `.gitignore` file, sorted shallowest-first; matched
    /// deepest-first so a nested file wins, as in git.
    gitignores: Vec<Gitignore>,
    warnings: Vec<String>,
}

impl Excluder {
    pub fn new(project_dir: &Path, docs_dir: &Path, patterns: &[String]) -> Excluder {
        let docs_dir = canonical(docs_dir);
        let repo_root = find_repo_root(&canonical(project_dir));
        let (gitignores, warnings) = match &repo_root {
            // Not a git repo, or a docs tree outside it: nothing is ignored.
            // Both are graceful, and the second is load-bearing — see
            // `collect_gitignores` on the panic this guards.
            Some(root) if docs_dir.starts_with(root) => collect_gitignores(root, &docs_dir),
            _ => (Vec::new(), Vec::new()),
        };
        Excluder {
            patterns: patterns.to_vec(),
            docs_dir,
            repo_root,
            gitignores,
            warnings,
        }
    }

    /// True when `rel` — a docs-dir-relative path to a **file** — is kept out.
    pub fn is_excluded(&self, rel: &Path) -> bool {
        // `exclude` first, so it wins over a gitignore `!negation`: it is
        // compositor's own config, and git has no say in overriding it.
        if matches_patterns(rel, &self.patterns) {
            return true;
        }
        // Lexical only — no disk access — so a `rel` that doesn't exist (an
        // on-demand asset request for a bogus URL) matches fine.
        self.is_ignored_under(&self.docs_dir, &self.docs_dir.join(rel))
    }

    /// True when an **absolute** path is hidden by a repo `.gitignore`.
    ///
    /// The gitignore half of `is_excluded`, for a path with no docs-relative
    /// form — an asset outside the docs tree, referenced by a repo-root README
    /// (see compositor's `root_assets`). The `exclude` patterns are
    /// docs-dir-relative by definition, so they do not apply here and are not
    /// consulted. Shares `is_ignored_under` with `is_excluded`, so git's
    /// directory-pruning rule holds identically on both paths.
    ///
    /// **Only the `.gitignore` files `new` collected are consulted** — the repo
    /// root, each directory between it and the docs dir, and each directory
    /// beneath the docs dir (see `collect_gitignores`). A `.gitignore` inside an
    /// *outside-docs* directory (a repo-root `images/.gitignore`) is not among
    /// them, so a rule only that file carries is not honored. The repo-root
    /// `.gitignore` — what actually hides scratch in practice — is.
    pub fn is_gitignored(&self, abs: &Path) -> bool {
        match &self.repo_root {
            Some(root) if abs.starts_with(root) => self.is_ignored_under(root, abs),
            // Not a git repo, or a path outside it: nothing is ignored.
            _ => false,
        }
    }

    /// Resolve `abs` against the matcher chain, applying git's directory-pruning
    /// rule with `base` as the top of the walk.
    ///
    /// Git's rule: "it is not possible to re-include a file if a parent
    /// directory of that file is excluded." Once an ancestor directory is itself
    /// ignored, git prunes the walk right there — it never reads a nested
    /// `.gitignore` inside it and never honors a deeper `!negation`, for the
    /// directory or anything under it. Walk `abs`'s ancestor directories
    /// shallow -> deep (`base`'s direct children down to the file's immediate
    /// parent) so the same short-circuit applies here: the moment one resolves
    /// to `Ignore`, the path is excluded regardless of what a nested rule would
    /// say.
    fn is_ignored_under(&self, base: &Path, abs: &Path) -> bool {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut cur = abs.parent();
        while let Some(d) = cur {
            if d == base || !d.starts_with(base) {
                break;
            }
            dirs.push(d.to_path_buf());
            cur = d.parent();
        }
        dirs.reverse(); // shallow -> deep, matching git's top-down traversal
        for dir in &dirs {
            if self.matched(dir, true) == Some(true) {
                return true;
            }
        }
        // No ancestor directory is ignored, so the path's own rule (including a
        // `!negation`) governs normally.
        self.matched(abs, false).unwrap_or(false)
    }

    /// Resolves `abs` against the matcher chain, deepest-first, honoring the
    /// first non-`None` result — git's own precedence. `Some(true)` = ignored,
    /// `Some(false)` = explicitly whitelisted (`!negation`), `None` = no rule
    /// matched anywhere in the chain.
    fn matched(&self, abs: &Path, is_dir: bool) -> Option<bool> {
        for gi in self.gitignores.iter().rev() {
            // Skip matchers this path isn't under: `matched_path_or_any_parents`
            // asserts the path is under the matcher root.
            if !abs.starts_with(gi.path()) {
                continue;
            }
            match gi.matched_path_or_any_parents(abs, is_dir) {
                Match::Ignore(_) => return Some(true),
                // An explicit `!negation` in the deepest matching file wins, as
                // in git — stop, don't fall through to a shallower rule.
                Match::Whitelist(_) => return Some(false),
                Match::None => {}
            }
        }
        None
    }

    /// Non-fatal `.gitignore` parse problems, for the CLI to print.
    ///
    /// A malformed `.gitignore` is lenient, unlike a malformed `compositor.toml`
    /// (a hard, named error): `.gitignore` is git's file, not compositor's
    /// config, and failing a docs build over a glob git itself tolerates would
    /// be hostile. `Gitignore::new` drops the bad glob and returns the rest.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// Falling back to the path as given when canonicalization fails silently
/// disables every gitignore rule for this `Excluder` — the `starts_with`
/// guards in `is_excluded`/`matched` then compare a non-canonical path
/// against canonical matcher roots (e.g. macOS `/tmp` vs `/private/tmp`) and
/// never match. That is inert today, not by construction: every real call
/// site constructs the `Excluder` and then immediately calls
/// `render_core::site::build_site`, whose own `WalkDir::new(docs_dir)` hard-
/// errors on the very first entry when `docs_dir` is unreadable or absent —
/// the same condition that would make `canonicalize` fail here. So a broken
/// canonicalization never survives to decide anything; the build stops
/// first. If a future call site ever uses an `Excluder` without going
/// through `build_site` right after, this fallback becomes a live way to
/// silently stop respecting `.gitignore`.
fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// True when `rel` falls under any exclude pattern. Each pattern is a path
/// prefix matched component-wise, so `superpowers` excludes `superpowers/x.md`
/// and any nesting below it, but NOT a sibling like `superpowers-notes.md`. A
/// trailing slash on a pattern is ignored.
fn matches_patterns(rel: &Path, patterns: &[String]) -> bool {
    patterns.iter().any(|p| {
        let pat = p.trim().trim_start_matches("./").trim_end_matches('/');
        !pat.is_empty() && rel.starts_with(pat)
    })
}

/// The nearest ancestor of `start` (inclusive) containing a `.git`.
///
/// Keying off `.git` rather than assuming `project_dir` is the repo root makes
/// "not a git repo -> nothing ignored" an explicit graceful state, and handles a
/// `project_dir` nested inside a repo whose governing `.gitignore` sits above it
/// (git honors it there).
///
/// `.git` is tested as an *entry*, not a directory: in a linked worktree it is a
/// **file** pointing at the real git dir.
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Every `.gitignore` that can govern a path under `docs_dir`: the repo root,
/// each directory between it and `docs_dir`, and each directory beneath it.
///
/// Deliberately not "every directory under `project_dir`" — that would walk
/// `target/`, which Cargo seeds with its own `.gitignore` containing `*`. This
/// only avoids that in practice when `docs_dir` is a subdir of `project_dir`
/// (the common case, e.g. `docs/`). In the no-config bare-Markdown-folder
/// default, `docs_dir == project_dir`, so the walk below descends `target/`,
/// `.git/`, and `.claude/worktrees/` the same as any other directory. That is
/// harmless, not a correctness gap: `build_site` walks that exact same tree
/// to render it, so collecting their `.gitignore` files costs nothing extra
/// and `target/`'s own `*` rule is simply one more (unreachable, since
/// `target/` sits outside any `docs_dir` a site would render) matcher.
///
/// A `.gitignore` *inside* an already-ignored directory is collected too.
/// That is wasted but harmless: `Excluder::is_excluded` prunes at the first
/// ignored ancestor directory before ever consulting a deeper matcher, so a
/// nested `.gitignore` built here for a path under an ignored directory is
/// built but never reached at match time.
fn collect_gitignores(repo_root: &Path, docs_dir: &Path) -> (Vec<Gitignore>, Vec<String>) {
    let mut dirs: Vec<PathBuf> = docs_dir
        .ancestors()
        .take_while(|d| d.starts_with(repo_root))
        .map(Path::to_path_buf)
        .collect();
    for entry in WalkDir::new(docs_dir).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_dir() && entry.path() != docs_dir {
            dirs.push(entry.into_path());
        }
    }

    let mut found = Vec::new();
    let mut warnings = Vec::new();
    for dir in dirs {
        let file = dir.join(".gitignore");
        if !file.is_file() {
            continue;
        }
        // `Gitignore::new` roots the matcher at the file's own parent, which is
        // what makes a nested `.gitignore` scope correctly. Do NOT swap this for
        // one `GitignoreBuilder` with several `add` calls: `add` parses patterns
        // relative to the *builder's* root, so a nested file's rules would be
        // silently mis-scoped to the repo root.
        let (gi, err) = Gitignore::new(&file);
        if let Some(e) = err {
            warnings.push(format!("{}: {e}", file.display()));
        }
        found.push(gi);
    }
    // Sort shallowest-first by depth so `.rev()` at match time is deepest-first.
    // WalkDir is depth-first, so its raw order interleaves depths across branches.
    found.sort_by_key(|gi| gi.path().components().count());
    (found, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A scratch project dir, cleaned first. `git` = whether to create the
    /// `.git` entry that makes repo-root discovery find it. Root discovery only
    /// needs the entry to exist — no `git init`, no git binary dependency.
    fn project(name: &str, git: bool) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("compositor-excl-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("docs")).unwrap();
        if git {
            fs::create_dir_all(d.join(".git")).unwrap();
        }
        d
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    // ---- `exclude` patterns (behaviour preserved from the free function) ----

    #[test]
    fn pattern_excludes_file_directly_under_dir() {
        let d = project("pat-direct", false);
        let e = Excluder::new(&d, &d.join("docs"), &["superpowers/".to_string()]);
        assert!(e.is_excluded(Path::new("superpowers/spec.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn pattern_excludes_nested_file() {
        let d = project("pat-nested", false);
        let e = Excluder::new(&d, &d.join("docs"), &["inbox".to_string()]);
        assert!(e.is_excluded(Path::new("inbox/archive/note.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn pattern_does_not_exclude_prefix_sibling() {
        let d = project("pat-sibling", false);
        let e = Excluder::new(&d, &d.join("docs"), &["superpowers".to_string()]);
        assert!(!e.is_excluded(Path::new("superpowers-notes.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn no_patterns_exclude_nothing() {
        let d = project("pat-empty", false);
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        assert!(!e.is_excluded(Path::new("inbox/note.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn pattern_leaves_unrelated_path_alone() {
        let d = project("pat-unrelated", false);
        let e = Excluder::new(
            &d,
            &d.join("docs"),
            &["superpowers/".to_string(), "inbox/".to_string()],
        );
        assert!(!e.is_excluded(Path::new("guides/watch.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn dot_slash_prefixed_pattern_still_excludes() {
        let d = project("pat-dotslash", false);
        let e = Excluder::new(&d, &d.join("docs"), &["./superpowers/".to_string()]);
        assert!(e.is_excluded(Path::new("superpowers/spec.md")));
        let _ = fs::remove_dir_all(&d);
    }

    // ---- gitignore ----

    #[test]
    fn gitignored_dir_is_excluded() {
        let d = project("gi-basic", true);
        write(&d, ".gitignore", "docs/superpowers/\n");
        write(&d, "docs/superpowers/note.md", "x");
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        assert!(e.is_excluded(Path::new("superpowers/note.md")));
        assert!(!e.is_excluded(Path::new("guides/watch.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn non_git_dir_excludes_nothing() {
        // A stray `.gitignore` in a non-repo means nothing to git, so it means
        // nothing here. This is the case a host app hits serving a bare
        // Markdown folder that was never a repo.
        let d = project("gi-nogit", false);
        write(&d, ".gitignore", "docs/superpowers/\n");
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        assert!(!e.is_excluded(Path::new("superpowers/note.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn nested_gitignore_is_scoped_to_its_own_dir() {
        // `GitignoreBuilder::add` parses patterns relative to the *builder's*
        // root, so a single root-rooted matcher would read `secret.md` as
        // repo-root-relative and wrongly hide docs/secret.md too. One matcher
        // per file is what makes this pass.
        let d = project("gi-nested", true);
        write(&d, ".gitignore", "/target\n");
        write(&d, "docs/sub/.gitignore", "secret.md\n");
        write(&d, "docs/sub/secret.md", "x");
        write(&d, "docs/secret.md", "x");
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        assert!(e.is_excluded(Path::new("sub/secret.md")));
        assert!(!e.is_excluded(Path::new("secret.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn negation_cannot_re_include_under_an_ignored_dir() {
        // Git: "it is not possible to re-include a file if a parent directory
        // of that file is excluded." `docs/gen/` is ignored by the root
        // `.gitignore`, so git prunes there — it never even reads the nested
        // `docs/gen/.gitignore`'s `!keep.md`, and `git check-ignore` reports
        // `gen/keep.md` as IGNORED (via the root rule), not whitelisted.
        let d = project("gi-negate", true);
        write(&d, ".gitignore", "docs/gen/\n");
        write(&d, "docs/gen/.gitignore", "!keep.md\n");
        write(&d, "docs/gen/keep.md", "x");
        write(&d, "docs/gen/other.md", "x");
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        assert!(e.is_excluded(Path::new("gen/keep.md")));
        assert!(e.is_excluded(Path::new("gen/other.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn negation_cannot_re_include_a_single_ignored_file() {
        // Same rule, the more common shape: a directory is ignored and a
        // later line in the *same* file tries to re-include one file inside
        // it. Git still prunes at the directory and never reaches the `!`
        // line — `git check-ignore -v` reports this IGNORED via the
        // `docs/priv/` rule, not the `!docs/priv/index.md` one.
        let d = project("gi-negate-single", true);
        write(&d, ".gitignore", "docs/priv/\n!docs/priv/index.md\n");
        write(&d, "docs/priv/index.md", "x");
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        assert!(e.is_excluded(Path::new("priv/index.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn negation_works_when_no_ancestor_directory_is_ignored() {
        // The positive case: git *does* honor a `!negation` when nothing
        // above the file is excluded. This must keep working — the fix for
        // the two tests above must not turn `!` into a no-op generally.
        let d = project("gi-negate-plain", true);
        write(&d, ".gitignore", "docs/*.md\n!docs/keep.md\n");
        write(&d, "docs/keep.md", "x");
        write(&d, "docs/other.md", "x");
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        assert!(!e.is_excluded(Path::new("keep.md")));
        assert!(e.is_excluded(Path::new("other.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn exclude_pattern_wins_over_gitignore_negation() {
        // `exclude` is compositor's own config; git's `!negation` has no say
        // over it (see the comment in `is_excluded`). The `.gitignore` here
        // has a real, applicable `!x/y.md` whitelist for this exact path
        // (no ignored ancestor directory is in play), so this isolates the
        // `exclude`-vs-`!` precedence specifically: without the `exclude`
        // wins-first rule, the whitelist would keep the file.
        let d = project("gi-negate-vs-exclude", true);
        write(&d, "docs/.gitignore", "!x/y.md\n");
        write(&d, "docs/x/y.md", "x");
        let e = Excluder::new(&d, &d.join("docs"), &["x/".to_string()]);
        assert!(e.is_excluded(Path::new("x/y.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn patterns_and_gitignore_compose_as_a_union() {
        // Two rules, two jobs: gitignore hides untracked scratch, `exclude`
        // hides a tracked tree that is kept in git but not published.
        let d = project("gi-union", true);
        write(&d, ".gitignore", "docs/scratch/\n");
        write(&d, "docs/scratch/a.md", "x");
        write(&d, "docs/tracked/b.md", "x");
        let e = Excluder::new(&d, &d.join("docs"), &["tracked/".to_string()]);
        assert!(e.is_excluded(Path::new("scratch/a.md")));
        assert!(e.is_excluded(Path::new("tracked/b.md")));
        assert!(!e.is_excluded(Path::new("guides/c.md")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn is_gitignored_judges_a_path_outside_the_docs_dir() {
        // A repo-root asset has no docs-relative path, so `is_excluded` cannot see
        // it. This is the check `RootAssets` uses for a README's images.
        let d = project("gi-abs", true);
        write(&d, ".gitignore", "scratch/\n");
        write(&d, "scratch/secret.png", "x");
        write(&d, "images/logo.png", "x");
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        let root = std::fs::canonicalize(&d).unwrap();
        assert!(e.is_gitignored(&root.join("scratch/secret.png")));
        assert!(!e.is_gitignored(&root.join("images/logo.png")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn is_gitignored_applies_gits_directory_pruning_rule() {
        // The negation has to live in a matcher `collect_gitignores` actually
        // gathers, or this proves nothing: a `.gitignore` in a sibling/outside-
        // docs directory (e.g. a top-level `scratch/.gitignore`) is never
        // collected at all, so a naive single-matcher `matched(abs, false)`
        // call would only ever see the root matcher and pass this test
        // identically with or without the ancestor-pruning walk.
        //
        // Put `docs_dir` a level below the repo root (`d/sub/docs`) so
        // `collect_gitignores` gathers BOTH `d/.gitignore` (root) and
        // `d/sub/.gitignore` (between root and docs_dir). The root file
        // ignores `sub/scratch/`; the deeper, collected `d/sub/.gitignore`
        // tries to whitelist `scratch/keep.png` inside it. Git still prunes at
        // the ignored ancestor directory and never reaches that deeper
        // `!negation` — without the shared `is_ignored_under` walk, a naive
        // deepest-matcher-wins lookup would hit `d/sub/.gitignore`'s whitelist
        // directly and wrongly re-include the file.
        let d = project("gi-abs-prune", true);
        write(&d, ".gitignore", "sub/scratch/\n");
        write(&d, "sub/.gitignore", "!scratch/keep.png\n");
        write(&d, "sub/scratch/keep.png", "x");
        write(&d, "sub/docs/index.md", "x");
        let e = Excluder::new(&d, &d.join("sub/docs"), &[]);
        let root = std::fs::canonicalize(&d).unwrap();
        assert!(
            e.is_gitignored(&root.join("sub/scratch/keep.png")),
            "a `!negation` in a deeper collected matcher must not re-include \
             a file under an ancestor directory the root matcher ignores"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn is_gitignored_is_false_outside_a_repo() {
        // Not a git repo -> nothing is ignored, the same graceful default `new` has.
        let d = project("gi-abs-nogit", false);
        write(&d, ".gitignore", "scratch/\n");
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        assert!(!e.is_gitignored(&d.join("scratch/secret.png")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn docs_dir_outside_repo_root_does_not_panic() {
        // Reachable via an absolute `docs_dir` in compositor.toml: `Path::join`
        // with an absolute arg discards the base, putting the docs tree outside
        // the repo. `matched_path_or_any_parents` *panics* on a path that
        // escapes its matcher root, so this must degrade to a no-match.
        let d = project("gi-outside", true);
        write(&d, ".gitignore", "docs/superpowers/\n");
        let elsewhere = project("gi-outside-docs", false);
        let e = Excluder::new(&d, &elsewhere, &[]);
        assert!(!e.is_excluded(Path::new("superpowers/note.md")));
        let _ = fs::remove_dir_all(&d);
        let _ = fs::remove_dir_all(&elsewhere);
    }
}
