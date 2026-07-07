use std::path::Path;
use anyhow::{Context, Result};
use render_core::site::build_site;
use crate::config::SiteConfig;
use crate::render_page::render_page;

pub fn run_build(project_dir: &Path) -> Result<()> {
    let cfg_path = project_dir.join("compositor.toml");
    let cfg: SiteConfig = toml::from_str(
        &std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("reading {}", cfg_path.display()))?,
    )?;

    let docs = project_dir.join(cfg.docs_dir());
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
