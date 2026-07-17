use anyhow::Result;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::exclude::Excluder;
use crate::frontmatter::split_frontmatter;
use crate::markdown::{first_h1, render_markdown, LinkPolicy, TocEntry};
use crate::nav::{tree_from_pages, NavTree};
use crate::wikilink::WikiIndex;

pub struct Page {
    pub rel_path: PathBuf,
    pub url: String,
    pub title: String,
    pub html: String,
    pub toc: Vec<TocEntry>,
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

pub fn build_site(docs_dir: &Path, policy: LinkPolicy, excluder: &Excluder) -> Result<SiteModel> {
    // Pass 1: collect metadata, known urls, and the wikilink index. Titles are
    // resolved here (frontmatter -> first H1 -> humanized stem) so the index can
    // carry each page's display title. Excluded paths are skipped.
    let mut raws = Vec::new(); // (rel, page_dir, url, title, body)
    let mut known_urls = std::collections::HashSet::new();
    let mut wiki = WikiIndex::new();
    for entry in WalkDir::new(docs_dir).sort_by_file_name() {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
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
        raws.push((rel, page_dir, url, title, body));
    }
    // Pass 2: render (md links and wikilinks now resolvable).
    let mut pages = Vec::new();
    for (rel, page_dir, url, title, body) in raws {
        let rendered = render_markdown(&body, &page_dir, &known_urls, &wiki, policy)?;
        pages.push(Page {
            url,
            rel_path: rel,
            title,
            html: rendered.html,
            toc: rendered.toc,
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

        let site = build_site(&d, LinkPolicy::Strict, &Excluder::new(&d, &d, &[])).unwrap();
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
        let err = build_site(&d, LinkPolicy::Strict, &Excluder::new(&d, &d, &[]))
            .err()
            .unwrap();
        assert!(err.to_string().contains("Nowhere"), "got: {err}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn build_serve_lenient_degrades_unresolvable_wikilink() {
        let d = scratch("lenient-dangling");
        write(&d, "index.md", "Dangling [[Nowhere]].\n");
        let site = build_site(&d, LinkPolicy::Lenient, &Excluder::new(&d, &d, &[])).unwrap();
        let home = &site.pages[0];
        assert!(
            home.html.contains(r#"data-wikilink="true""#),
            "got: {}",
            home.html
        );
        let _ = fs::remove_dir_all(&d);
    }
}
