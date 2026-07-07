use std::path::{Component, Path};
use anyhow::{anyhow, Context, Result};
use render_core::site::build_site;
use crate::config::SiteConfig;
use crate::render_page::render_page;

pub fn run_build(project_dir: &Path) -> Result<()> {
    let cfg_path = project_dir.join("compositor.toml");
    let cfg: SiteConfig = toml::from_str(
        &std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("reading {}", cfg_path.display()))?,
    )
    .with_context(|| format!("parsing {}", cfg_path.display()))?;

    let docs = project_dir.join(cfg.docs_dir());
    validate_out_dir(cfg.out_dir())?;
    let out = project_dir.join(cfg.out_dir());
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;

    let site = build_site(&docs)?;
    for page in &site.pages {
        let html = render_page(&cfg, &site.nav, page);
        let dest = out.join(&page.url);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, html)?;
    }

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

fn run_pagefind(out: &Path) {
    match std::process::Command::new("pagefind")
        .arg("--site").arg(out)
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("warning: pagefind exited with {s}"),
        Err(_) => eprintln!("warning: pagefind not found on PATH; search index skipped"),
    }
}
