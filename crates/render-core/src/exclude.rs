use std::path::Path;

/// True when `rel` (a docs-dir-relative path) falls under any exclude pattern.
/// Each pattern is a path prefix matched component-wise, so `superpowers`
/// excludes `superpowers/x.md` and any nesting below it, but NOT a sibling like
/// `superpowers-notes.md`. A trailing slash on a pattern is ignored.
pub fn is_excluded(rel: &Path, patterns: &[String]) -> bool {
    patterns.iter().any(|p| {
        let pat = p.trim().trim_start_matches("./").trim_end_matches('/');
        !pat.is_empty() && rel.starts_with(pat)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn excludes_file_directly_under_dir() {
        let pats = vec!["superpowers/".to_string()];
        assert!(is_excluded(Path::new("superpowers/spec.md"), &pats));
    }

    #[test]
    fn excludes_nested_file() {
        let pats = vec!["inbox".to_string()];
        assert!(is_excluded(Path::new("inbox/archive/note.md"), &pats));
    }

    #[test]
    fn does_not_exclude_prefix_sibling() {
        let pats = vec!["superpowers".to_string()];
        assert!(!is_excluded(Path::new("superpowers-notes.md"), &pats));
    }

    #[test]
    fn empty_patterns_exclude_nothing() {
        assert!(!is_excluded(Path::new("inbox/note.md"), &[]));
    }

    #[test]
    fn unrelated_path_not_excluded() {
        let pats = vec!["superpowers/".to_string(), "inbox/".to_string()];
        assert!(!is_excluded(Path::new("guides/watch.md"), &pats));
    }

    #[test]
    fn dot_slash_prefixed_pattern_still_excludes() {
        let pats = vec!["./superpowers/".to_string()];
        assert!(is_excluded(Path::new("superpowers/spec.md"), &pats));
    }
}
