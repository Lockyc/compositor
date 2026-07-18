use anyhow::Result;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::exclude::Excluder;
use crate::frontmatter::split_frontmatter;
use crate::markdown::{first_h1, render_markdown, render_markdown_editable, LinkPolicy, TocEntry};
use crate::nav::{tree_from_pages, NavTree};
use crate::wikilink::WikiIndex;

pub struct Page {
    pub rel_path: PathBuf,
    pub url: String,
    pub title: String,
    pub html: String,
    pub toc: Vec<TocEntry>,
    pub edit_source: Option<EditSource>,
}

/// Everything a serve-mode client needs to map a rendered block back to a real
/// file line and slice the verbatim source for an inline edit. Populated in
/// edit mode only: by `build_site` (`edit = true`) for docs-tree pages, and by
/// the repo-root surfacing paths in `compositor::render_page` (`resolve_home`'s
/// repo-README tier, `surface_repo_agent_files`) for the repo-root README,
/// CLAUDE, and AGENTS pages.
pub struct EditSource {
    /// The page's full original file, frontmatter included.
    pub source: String,
    /// The number of lines `split_frontmatter` stripped off `source` before
    /// rendering (0 for a page with no frontmatter block).
    pub fm_lines: usize,
    /// Task 2's per-output-line map for the body: `Some(source_line_idx)` for
    /// a passthrough line, `None` for a line synthesized by the admonition
    /// preprocessor. Faithful and unprocessed — a `None` region's line count
    /// need not match its source span (see admonitions.rs), so any
    /// position -> file-line conversion must go through this map per output
    /// line rather than assuming a constant offset.
    pub line_map: Vec<Option<usize>>,
    /// The absolute on-disk file this page renders from — the exact path
    /// `/__edit` writes when the page is edited. Single-sources the write
    /// target so a page whose source lives outside the docs dir (a repo-root
    /// README/CLAUDE/AGENTS) is addressable, not only docs-tree pages.
    pub path: PathBuf,
}

pub struct SiteModel {
    pub pages: Vec<Page>,
    pub nav: NavTree,
}

pub fn humanize_filename(stem: &str) -> String {
    stem.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn url_for(rel: &Path) -> String {
    let mut s = rel.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = s.strip_suffix(".md") {
        s = format!("{stripped}.html");
    }
    s
}

pub fn build_site(
    docs_dir: &Path,
    policy: LinkPolicy,
    excluder: &Excluder,
    edit: bool,
) -> Result<SiteModel> {
    // Pass 1: collect metadata, known urls, and the wikilink index. Titles are
    // resolved here (frontmatter -> first H1 -> humanized stem) so the index can
    // carry each page's display title. Excluded paths are skipped.
    let mut raws = Vec::new(); // (rel, page_dir, url, title, raw, body)
    let mut known_urls = std::collections::HashSet::new();
    let mut assets = std::collections::HashSet::new();
    let mut wiki = WikiIndex::new();
    for entry in WalkDir::new(docs_dir).sort_by_file_name() {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            // Non-Markdown files are the site's assets. Collect them (same
            // Excluder as the pages) so images can be validated in pass 2;
            // compositor's `copy_assets` is what actually mirrors them out.
            if entry.file_type().is_file() {
                let rel = path.strip_prefix(docs_dir)?;
                if !excluder.is_excluded(rel) {
                    assets.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            continue;
        }
        let rel = path.strip_prefix(docs_dir)?.to_path_buf();
        if excluder.is_excluded(&rel) {
            continue;
        }
        let raw = std::fs::read_to_string(path)?;
        let (fm, body) = split_frontmatter(&raw);
        let page_dir = rel.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("page")
            .to_string();
        let url = url_for(&rel);
        let title = fm
            .title
            .clone()
            .or_else(|| first_h1(&body))
            .unwrap_or_else(|| humanize_filename(&stem));
        known_urls.insert(url.clone());
        wiki.add_page(&url, &title, &rel, &stem, &fm.aliases);
        raws.push((rel, page_dir, url, title, raw, body));
    }
    // Pass 2: render (md links, wikilinks, and images now resolvable).
    let images = crate::markdown::DocsAssets::new(assets, policy);
    let mut pages = Vec::new();
    for (rel, page_dir, url, title, raw, body) in raws {
        let (rendered, edit_source) = if edit {
            let (rendered, line_map) =
                render_markdown_editable(&body, &page_dir, &known_urls, &wiki, policy, &images)?;
            // `body` is a true suffix of `raw` (`split_frontmatter` either
            // returns `raw` unchanged or a slice starting after the closing
            // `---` line), so the byte offset where it starts is exactly the
            // frontmatter-plus-delimiters prefix; counting that prefix's lines
            // gives the number `split_frontmatter` stripped.
            let fm_lines = raw[..raw.len() - body.len()].lines().count();
            (
                rendered,
                Some(EditSource {
                    source: raw,
                    fm_lines,
                    line_map,
                    path: docs_dir.join(&rel),
                }),
            )
        } else {
            (
                render_markdown(&body, &page_dir, &known_urls, &wiki, policy, &images)?,
                None,
            )
        };
        pages.push(Page {
            url,
            rel_path: rel,
            title,
            html: rendered.html,
            toc: rendered.toc,
            edit_source,
        });
    }
    let nav = tree_from_pages(&pages);
    Ok(SiteModel { pages, nav })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::LinkPolicy;
    use std::fs;

    /// A fresh, empty temp dir unique to this test name + process, cleaned first.
    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "compositor-wikilink-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    #[test]
    fn build_resolves_wikilink_by_title_with_target_title_text() {
        let d = scratch("by-title");
        write(
            &d,
            "guide/getting-started.md",
            "---\ntitle: Getting Started\n---\n# Getting Started\n",
        );
        write(&d, "index.md", "Welcome — see [[Getting Started]].\n");

        let site = build_site(&d, LinkPolicy::Strict, &Excluder::new(&d, &d, &[]), false).unwrap();
        let home = site.pages.iter().find(|p| p.url == "index.html").unwrap();
        assert!(
            home.html
                .contains(r#"<a href="guide/getting-started.html">Getting Started</a>"#),
            "got: {}",
            home.html
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn build_errors_on_unresolvable_wikilink_under_strict() {
        let d = scratch("strict-dangling");
        write(&d, "index.md", "Dangling [[Nowhere]].\n");
        let err = build_site(&d, LinkPolicy::Strict, &Excluder::new(&d, &d, &[]), false)
            .err()
            .unwrap();
        assert!(err.to_string().contains("Nowhere"), "got: {err}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn build_serve_lenient_degrades_unresolvable_wikilink() {
        let d = scratch("lenient-dangling");
        write(&d, "index.md", "Dangling [[Nowhere]].\n");
        let site = build_site(&d, LinkPolicy::Lenient, &Excluder::new(&d, &d, &[]), false).unwrap();
        let home = &site.pages[0];
        assert!(
            home.html.contains(r#"data-wikilink="true""#),
            "got: {}",
            home.html
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn edit_mode_carries_source_and_emits_sourcepos() {
        let tmp = std::env::temp_dir().join(format!("rc-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("index.md"), "---\ntitle: T\n---\n# H\n\npara\n").unwrap();
        let ex = Excluder::new(&tmp, &tmp, &[]);

        let site = build_site(&tmp, LinkPolicy::Lenient, &ex, true).unwrap();
        let page = site.pages.iter().find(|p| p.url == "index.html").unwrap();
        let es = page
            .edit_source
            .as_ref()
            .expect("edit mode populates edit_source");
        assert!(
            es.source.starts_with("---\ntitle: T\n---\n"),
            "full source incl frontmatter"
        );
        assert_eq!(es.fm_lines, 3, "3 frontmatter lines stripped");
        // comrak stamped positions on the rendered blocks.
        assert!(page.html.contains("data-sourcepos"), "html: {}", page.html);

        // Non-edit mode leaves it clean (build output).
        let plain = build_site(&tmp, LinkPolicy::Lenient, &ex, false).unwrap();
        let pp = plain.pages.iter().find(|p| p.url == "index.html").unwrap();
        assert!(pp.edit_source.is_none());
        assert!(
            !pp.html.contains("data-sourcepos"),
            "build html must be clean: {}",
            pp.html
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn edit_source_carries_absolute_source_path() {
        let dir = scratch("editsource-path");
        write(&dir, "guide.md", "# Guide\n\ntext\n");
        let ex = Excluder::new(&dir, &dir, &[]);
        let site = build_site(&dir, LinkPolicy::Lenient, &ex, true).unwrap();
        let page = site.pages.iter().find(|p| p.url == "guide.html").unwrap();
        let es = page
            .edit_source
            .as_ref()
            .expect("edit mode populates edit_source");
        assert_eq!(es.path, dir.join("guide.md"));
        let _ = fs::remove_dir_all(&dir);
    }
}
