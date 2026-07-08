use crate::config::SiteConfig;
use crate::render_page::render_page;
use anyhow::{anyhow, Context, Result};
use render_core::site::build_site;
use render_core::LinkPolicy;
use std::path::{Component, Path};
use walkdir::WalkDir;

pub fn run_build(project_dir: &Path) -> Result<()> {
    let cfg = SiteConfig::load(project_dir)?;
    let docs = cfg.docs_path(project_dir);
    validate_out_dir(cfg.out_dir())?;
    let out = project_dir.join(cfg.out_dir());
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;

    let site = build_site(&docs, LinkPolicy::Strict)?;
    for page in &site.pages {
        let html = render_page(&cfg, &site.nav, page);
        let dest = out.join(&page.url);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, html)?;
    }
    copy_assets(&docs, &out)?;

    run_pagefind(&out);
    println!("built {} pages -> {}", site.pages.len(), out.display());
    Ok(())
}

/// Guard against an `out_dir` that would make `remove_dir_all(out)` delete the
/// project directory itself (or an ancestor of it) — e.g. a misconfigured
/// `out_dir = "."`, `".."`, or `""`.
fn validate_out_dir(out_dir: &str) -> Result<()> {
    if out_dir.trim().is_empty() {
        return Err(anyhow!("out_dir must not be empty"));
    }
    let path = Path::new(out_dir);
    let mut components = path.components().peekable();
    if components.peek().is_none() {
        return Err(anyhow!("out_dir {out_dir:?} is invalid"));
    }
    for component in components {
        match component {
            // "." alone (or repeated, e.g. "./.") never descends anywhere,
            // so it normalizes to the project dir itself.
            Component::CurDir => {
                return Err(anyhow!("out_dir must not be \".\""));
            }
            // Any ".." component can walk back out of the project dir and
            // into an ancestor.
            Component::ParentDir => {
                return Err(anyhow!(
                    "out_dir {out_dir:?} must not contain a \"..\" component"
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Mirror every non-Markdown file in `docs` into `out`, preserving relative
/// paths. Markdown is rendered to HTML elsewhere; everything else (images,
/// downloads, data files a page links to) is copied verbatim so those
/// references resolve in the built site — matching MkDocs, which copies all
/// non-doc files from the docs dir into the output.
fn copy_assets(docs: &Path, out: &Path) -> Result<()> {
    // Skip the output tree: when the docs dir *is* the project dir (a bare
    // Markdown folder with no config), `out` sits inside `docs`, so without
    // this the walk would copy the freshly-written site back into itself.
    for entry in WalkDir::new(docs)
        .into_iter()
        .filter_entry(|e| !e.path().starts_with(out))
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            continue;
        }
        let rel = path.strip_prefix(docs)?;
        let dest = out.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(path, &dest).with_context(|| format!("copying asset {}", path.display()))?;
    }
    Ok(())
}

fn run_pagefind(out: &Path) {
    match std::process::Command::new("pagefind")
        .arg("--site")
        .arg(out)
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("warning: pagefind exited with {s}"),
        Err(_) => eprintln!("warning: pagefind not found on PATH; search index skipped"),
    }
}
