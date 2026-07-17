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
    /// One matcher per `.gitignore` file, sorted shallowest-first; matched
    /// deepest-first so a nested file wins, as in git.
    gitignores: Vec<Gitignore>,
    warnings: Vec<String>,
}

impl Excluder {
    pub fn new(project_dir: &Path, docs_dir: &Path, patterns: &[String]) -> Excluder {
        let docs_dir = canonical(docs_dir);
        let (gitignores, warnings) = match find_repo_root(&canonical(project_dir)) {
            // Not a git repo, or a docs tree outside it: nothing is ignored.
            // Both are graceful, and the second is load-bearing — see
            // `collect_gitignores` on the panic this guards.
            Some(root) if docs_dir.starts_with(&root) => collect_gitignores(&root, &docs_dir),
            _ => (Vec::new(), Vec::new()),
        };
        Excluder {
            patterns: patterns.to_vec(),
            docs_dir,
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
        let abs = self.docs_dir.join(rel);
        for gi in self.gitignores.iter().rev() {
            // Skip matchers this path isn't under: `matched_path_or_any_parents`
            // asserts the path is under the matcher root.
            if !abs.starts_with(gi.path()) {
                continue;
            }
            match gi.matched_path_or_any_parents(&abs, /* is_dir */ false) {
                Match::Ignore(_) => return true,
                // An explicit `!negation` in the deepest matching file wins, as
                // in git — stop, don't fall through to a shallower rule.
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
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
/// Deliberately not "every directory under `project_dir`" — that walks `target/`,
/// which Cargo seeds with its own `.gitignore` containing `*`.
///
/// A `.gitignore` *inside* an already-ignored directory is collected too. That is
/// wasted but harmless: everything under it is excluded by the ancestor rule
/// regardless, and pruning would need the matchers we are still building.
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
    fn negation_in_deeper_file_wins() {
        let d = project("gi-negate", true);
        write(&d, ".gitignore", "docs/gen/\n");
        write(&d, "docs/gen/.gitignore", "!keep.md\n");
        write(&d, "docs/gen/keep.md", "x");
        write(&d, "docs/gen/other.md", "x");
        let e = Excluder::new(&d, &d.join("docs"), &[]);
        assert!(!e.is_excluded(Path::new("gen/keep.md")));
        assert!(e.is_excluded(Path::new("gen/other.md")));
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
