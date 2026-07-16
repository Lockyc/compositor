//! Wikilink resolution: an Obsidian-style `[[name]]` references a page by name,
//! not path. This module builds a tree-wide name -> target index and resolves a
//! typed name against it. Pure logic; no disk or CLI assumptions.

use std::collections::HashMap;
use std::path::Path;

/// Number of precedence tiers a page registers keys at (lower index wins):
/// 0 = full rel-path stem, 1 = frontmatter title, 2 = alias, 3 = stem/humanized.
const TIERS: usize = 4;

/// A resolved wikilink target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiTarget {
    /// Site-root-relative url, e.g. "guide/setup.html".
    pub url: String,
    /// The target page's canonical title (bare-link display text).
    pub title: String,
}

/// The outcome of resolving a typed wikilink name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WikiResolution {
    /// Exactly one page matched at the winning tier.
    Resolved(WikiTarget),
    /// Two or more distinct pages matched at the winning tier. Candidates keep
    /// registration (WalkDir) order; `[0]` is the deterministic lenient pick.
    Ambiguous(Vec<WikiTarget>),
    /// No page matched.
    Unresolved,
}

/// Name -> per-tier candidate lists.
#[derive(Default)]
pub struct WikiIndex {
    map: HashMap<String, [Vec<WikiTarget>; TIERS]>,
}

impl WikiIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a page's resolution keys. Call once per page, in WalkDir order,
    /// so ambiguous candidates keep a deterministic order.
    pub fn add_page(
        &mut self,
        url: &str,
        title: &str,
        rel_path: &Path,
        stem: &str,
        aliases: &[String],
    ) {
        let target = WikiTarget {
            url: url.to_string(),
            title: title.to_string(),
        };
        // tier 0: full rel-path stem (e.g. "guide/setup") — path-qualified links
        // only. A root page's rel-path stem IS its bare name (no directory), so
        // registering it at tier 0 would put a root page's bare name at the
        // highest-precedence tier, wrongly outranking another page's frontmatter
        // title (tier 1) on the same key and silently suppressing the strict
        // "ambiguous" error. A root page needs no tier-0 entry: `[[name]]` for it
        // already resolves via tier 3 (stem/humanized stem).
        if rel_path.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
            let path_stem = rel_path.with_extension("");
            self.insert(&normalize_key(&path_stem.to_string_lossy()), 0, &target);
        }
        // tier 1: canonical title.
        self.insert(&normalize_key(title), 1, &target);
        // tier 2: aliases.
        for a in aliases {
            self.insert(&normalize_key(a), 2, &target);
        }
        // tier 3: file stem + humanized stem.
        self.insert(&normalize_key(stem), 3, &target);
        self.insert(
            &normalize_key(&crate::site::humanize_filename(stem)),
            3,
            &target,
        );
    }

    fn insert(&mut self, key: &str, tier: usize, target: &WikiTarget) {
        if key.is_empty() {
            return;
        }
        let tiers = self.map.entry(key.to_string()).or_default();
        // Dedupe by url within a tier: one page can produce the same key at one
        // tier twice (stem == humanized stem, or a root page's path == its stem),
        // and that must not read as a collision.
        if tiers[tier].iter().any(|t| t.url == target.url) {
            return;
        }
        tiers[tier].push(target.clone());
    }

    /// Resolve a typed name (the caller has already split off any `#anchor`).
    pub fn resolve(&self, name: &str) -> WikiResolution {
        let Some(tiers) = self.map.get(&normalize_key(name)) else {
            return WikiResolution::Unresolved;
        };
        for tier in tiers {
            match tier.len() {
                0 => continue,
                1 => return WikiResolution::Resolved(tier[0].clone()),
                _ => return WikiResolution::Ambiguous(tier.clone()),
            }
        }
        WikiResolution::Unresolved
    }
}

/// Normalize a wikilink key: lowercase, trim, collapse internal whitespace runs
/// to a single space. Slashes are preserved (path-qualified keys keep them).
/// The caller passes comrak's cleaned url directly — `clean_url` trims and
/// unescapes HTML entities/backslashes but does not percent-encode, so the url
/// is already the plain typed name.
pub fn normalize_key(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A page-relative href from a page's directory to a site-root-relative target
/// url — e.g. from `guide` to `admin/setup.html` yields `../admin/setup.html`.
/// Mirrors the existing md-link behavior so the site works under a hosting subpath.
pub fn relative_url(from_dir: &Path, to_url: &str) -> String {
    let from: Vec<&str> = from_dir
        .to_str()
        .unwrap_or("")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let to: Vec<&str> = to_url.split('/').filter(|s| !s.is_empty()).collect();
    let to_dirs = &to[..to.len().saturating_sub(1)];
    let mut common = 0;
    while common < from.len() && common < to_dirs.len() && from[common] == to_dirs[common] {
        common += 1;
    }
    let ups = from.len() - common;
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), ups).collect();
    parts.extend(to[common..].iter().map(|s| s.to_string()));
    if parts.is_empty() {
        to.last().map(|s| s.to_string()).unwrap_or_default()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn idx() -> WikiIndex {
        // guide/getting-started.md  title "Getting Started"
        // admin/setup.md            title "Admin Setup", alias "install"
        // reference/setup.md        title unrelated to its stem ("Reference
        //                           Notes"); only its stem is "setup", so it
        //                           collides with admin/setup at tier 3, not
        //                           tier 1.
        let mut w = WikiIndex::new();
        w.add_page(
            "guide/getting-started.html",
            "Getting Started",
            &PathBuf::from("guide/getting-started.md"),
            "getting-started",
            &[],
        );
        w.add_page(
            "admin/setup.html",
            "Admin Setup",
            &PathBuf::from("admin/setup.md"),
            "setup",
            &["install".to_string()],
        );
        w.add_page(
            "reference/setup.html",
            "Reference Notes",
            &PathBuf::from("reference/setup.md"),
            "setup",
            &[],
        );
        // notes/quick-tips.md title "Notes" (unrelated to its stem) — isolates the
        // humanized-stem (tier 3) registration: unlike getting-started above,
        // nothing else registers "quick tips" at a higher tier, so a query of the
        // humanized form actually exercises tier 3 rather than being masked by a
        // matching title.
        w.add_page(
            "notes/quick-tips.html",
            "Notes",
            &PathBuf::from("notes/quick-tips.md"),
            "quick-tips",
            &[],
        );
        w
    }

    #[test]
    fn normalize_lowercases_trims_and_collapses_whitespace() {
        assert_eq!(normalize_key("  Getting   Started "), "getting started");
        assert_eq!(normalize_key("GUIDE/Setup"), "guide/setup");
    }

    #[test]
    fn resolves_by_title_case_insensitively() {
        match idx().resolve("getting started") {
            WikiResolution::Resolved(t) => {
                assert_eq!(t.url, "guide/getting-started.html");
                assert_eq!(t.title, "Getting Started");
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn resolves_by_stem_and_humanized_stem() {
        // Raw hyphen stem: resolves directly via tier 3.
        assert!(matches!(
            idx().resolve("getting-started"),
            WikiResolution::Resolved(t) if t.url == "guide/getting-started.html"
        ));
        // Humanized form, on a page whose title does NOT also match it (unlike
        // "getting-started" -> "Getting Started", which is masked by the tier-1
        // title on the same fixture) — isolates the tier-3 humanized-stem
        // registration itself.
        assert!(matches!(
            idx().resolve("quick tips"),
            WikiResolution::Resolved(t) if t.url == "notes/quick-tips.html"
        ));
    }

    #[test]
    fn title_tier_beats_stem_tier_no_collision() {
        // "setup": admin/setup has ALIAS "setup" is wrong — here the deciding case is
        // the frontmatter title of a DIFFERENT page winning over a stem. Model it:
        let mut w = WikiIndex::new();
        // Page A: title exactly "Setup" (tier 1).
        w.add_page("a.html", "Setup", &PathBuf::from("a.md"), "alpha", &[]);
        // Page B: stem "setup" (tier 3), unrelated title.
        w.add_page("b.html", "Bravo", &PathBuf::from("b.md"), "setup", &[]);
        match w.resolve("setup") {
            WikiResolution::Resolved(t) => assert_eq!(t.url, "a.html"),
            other => panic!("title should win over stem, got {other:?}"),
        }
    }

    #[test]
    fn two_stems_collide_is_ambiguous_in_walk_order() {
        // admin/setup and reference/setup both have stem "setup", no title match.
        match idx().resolve("setup") {
            WikiResolution::Ambiguous(cands) => {
                let urls: Vec<&str> = cands.iter().map(|c| c.url.as_str()).collect();
                assert_eq!(urls, vec!["admin/setup.html", "reference/setup.html"]);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn alias_resolves_to_its_page() {
        assert!(matches!(
            idx().resolve("install"),
            WikiResolution::Resolved(t) if t.url == "admin/setup.html"
        ));
    }

    #[test]
    fn path_qualified_resolves_exact_rel_path_stem() {
        assert!(matches!(
            idx().resolve("reference/setup"),
            WikiResolution::Resolved(t) if t.url == "reference/setup.html"
        ));
    }

    #[test]
    fn unknown_name_is_unresolved() {
        assert!(matches!(idx().resolve("nope"), WikiResolution::Unresolved));
    }

    #[test]
    fn same_page_same_key_twice_is_not_a_collision() {
        // A one-word stem: stem == humanized stem after normalization ("setup"),
        // so the page registers "setup" at tier 3 twice — must dedupe, not collide.
        let mut w = WikiIndex::new();
        w.add_page("only.html", "Only", &PathBuf::from("only.md"), "setup", &[]);
        assert!(matches!(
            w.resolve("setup"),
            WikiResolution::Resolved(t) if t.url == "only.html"
        ));
    }

    #[test]
    fn relative_url_computes_page_relative_href() {
        assert_eq!(
            relative_url(Path::new("guide"), "admin/setup.html"),
            "../admin/setup.html"
        );
        assert_eq!(
            relative_url(Path::new(""), "guide/setup.html"),
            "guide/setup.html"
        );
        assert_eq!(
            relative_url(Path::new("guide"), "guide/setup.html"),
            "setup.html"
        );
        assert_eq!(relative_url(Path::new("a/b"), "a/c.html"), "../c.html");
    }
}
