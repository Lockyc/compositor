use std::path::{Path, PathBuf};
use anyhow::Result;
use walkdir::WalkDir;

use crate::frontmatter::split_frontmatter;
use crate::markdown::render_markdown;

pub struct Page {
    pub rel_path: PathBuf,
    pub url: String,
    pub title: String,
    pub html: String,
}

pub struct SiteModel {
    pub pages: Vec<Page>,
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

pub fn build_site(docs_dir: &Path) -> Result<SiteModel> {
    let mut pages = Vec::new();
    for entry in WalkDir::new(docs_dir).sort_by_file_name() {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let raw = std::fs::read_to_string(path)?;
        let (fm, body) = split_frontmatter(&raw);
        let rendered = render_markdown(&body);
        let rel = path.strip_prefix(docs_dir)?.to_path_buf();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
        let title = fm
            .title
            .or(rendered.first_h1)
            .unwrap_or_else(|| humanize_filename(stem));
        pages.push(Page {
            url: url_for(&rel),
            rel_path: rel,
            title,
            html: rendered.html,
        });
    }
    Ok(SiteModel { pages })
}
