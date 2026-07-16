use anyhow::Result;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::exclude::is_excluded;
use crate::frontmatter::split_frontmatter;
use crate::markdown::{render_markdown, LinkPolicy, TocEntry};
use crate::nav::{tree_from_pages, NavTree};

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

pub fn build_site(docs_dir: &Path, policy: LinkPolicy, exclude: &[String]) -> Result<SiteModel> {
    // Pass 1: collect page metadata + known urls.
    let mut raws = Vec::new(); // (rel, page_dir, stem, fm_title, body)
    let mut known_urls = std::collections::HashSet::new();
    for entry in WalkDir::new(docs_dir).sort_by_file_name() {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let rel = path.strip_prefix(docs_dir)?.to_path_buf();
        if is_excluded(&rel, exclude) {
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
        known_urls.insert(url_for(&rel));
        raws.push((rel, page_dir, stem, fm.title, body));
    }
    // Pass 2: render (links now resolvable).
    let mut pages = Vec::new();
    for (rel, page_dir, stem, fm_title, body) in raws {
        let rendered = render_markdown(&body, &page_dir, &known_urls, policy)?;
        let title = fm_title
            .clone()
            .or_else(|| rendered.first_h1.clone())
            .unwrap_or_else(|| humanize_filename(&stem));
        pages.push(Page {
            url: url_for(&rel),
            rel_path: rel,
            title,
            html: rendered.html,
            toc: rendered.toc,
        });
    }
    let nav = tree_from_pages(&pages);
    Ok(SiteModel { pages, nav })
}
