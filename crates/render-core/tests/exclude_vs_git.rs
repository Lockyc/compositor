//! `Excluder` against git itself: for generated `.gitignore` layouts, every file
//! is excluded exactly when `git check-ignore` says it is ignored, plus any path
//! with a `.git` component, which is never published.

use proptest::prelude::*;
use render_core::Excluder;
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const ALPHABET: &[&str] = &[
    "dir/",
    "*.md",
    "!keep.md",
    "/x",
    "**/y",
    "./z",
    "a/",
    "!a/",
    "*.txt",
    "!dir/",
    "y",
    "/docs/x",
    "keep.md",
    "sub/",
    "!sub/keep.md",
];

/// Docs-relative files written to disk, so git sees real files and directories.
const FILES: &[&str] = &[
    "a.md",
    "keep.md",
    "x/c.md",
    "y/d.md",
    "z/e.md",
    "dir/keep.md",
    "dir/g.txt",
    "a/b.md",
    "a/keep.md",
    "a/x",
    "a/y",
    "a/z",
    "a/dir/keep.md",
    "a/dir/h.md",
    "a/sub/keep.md",
    "a/sub/y/i.md",
];

/// Docs-relative `.git` paths, asked of the `Excluder` only. Creating them would
/// make git treat the docs dir as a nested repo and change its own answers.
const GIT_PATHS: &[&str] = &[".git/config", "a/.git", "a/.git/HEAD", "dir/.git/x.md"];

fn scratch() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "compositor-exclgit-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn git(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "core.excludesFile=/dev/null"]);
    c
}

/// The repo-relative paths git reports as ignored.
fn git_ignored(root: &Path, paths: &[String]) -> BTreeSet<String> {
    let mut child = git(root)
        .args(["check-ignore", "--no-index", "--stdin", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for p in paths {
            stdin.write_all(p.as_bytes()).unwrap();
            stdin.write_all(b"\0").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    // 0 = some ignored, 1 = none ignored; anything else is a git error.
    assert!(
        matches!(out.status.code(), Some(0 | 1)),
        "git check-ignore failed: {out:?}"
    );
    out.stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8(s.to_vec()).unwrap())
        .collect()
}

fn patterns() -> impl Strategy<Value = Vec<&'static str>> {
    prop::collection::vec(prop::sample::select(ALPHABET), 0..5)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn excluder_agrees_with_git_check_ignore(
        root_rules in patterns(),
        nested_rules in patterns(),
        docs_is_subdir in any::<bool>(),
    ) {
        let root = scratch();
        let status = git(&root).args(["init", "-q"]).status().unwrap();
        prop_assert!(status.success());
        let docs_rel = if docs_is_subdir { "docs/" } else { "" };
        let docs = root.join(docs_rel);
        for f in FILES {
            let p = docs.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        }
        fs::write(root.join(".gitignore"), root_rules.join("\n") + "\n").unwrap();
        fs::write(docs.join("a/.gitignore"), nested_rules.join("\n") + "\n").unwrap();

        let repo_paths: Vec<String> = FILES.iter().map(|f| format!("{docs_rel}{f}")).collect();
        let ignored = git_ignored(&root, &repo_paths);
        let excluder = Excluder::new(&root, &docs, &[]);

        for (f, repo_path) in FILES.iter().zip(&repo_paths) {
            let by_git = ignored.contains(repo_path);
            prop_assert_eq!(
                excluder.is_excluded(Path::new(f)),
                by_git,
                "{} (root .gitignore {:?}, a/.gitignore {:?})",
                repo_path,
                root_rules,
                nested_rules
            );
        }
        for g in GIT_PATHS {
            prop_assert!(excluder.is_excluded(Path::new(g)), "{} must be excluded", g);
        }
        fs::remove_dir_all(&root).ok();
    }
}
