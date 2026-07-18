use anyhow::{Context, Result};
use render_core::site::humanize_filename;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
pub struct SiteConfig {
    pub site_name: String,
    #[serde(default)]
    pub site_url: Option<String>,
    #[serde(default)]
    pub repo_url: Option<String>,
    #[serde(default)]
    pub docs_dir: Option<String>,
    #[serde(default)]
    pub out_dir: Option<String>,
    /// Docs-dir-relative path prefixes to skip when rendering and copying assets
    /// (the `exclude_docs` analog). E.g. `["superpowers/"]`, `["inbox/"]`.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Surface a repo-root `CLAUDE.md` as a top-level nav entry. Default `true`
    /// (a missing key and a no-config directory both resolve on). Set `false` to
    /// hide it. `Option` so "unset" means on in both the deserialize and the
    /// synthesized (`..Default::default()`) paths — a plain `bool` would take the
    /// derived-`Default` `false` and silently disable surfacing for bare dirs.
    #[serde(default)]
    pub surface_claude_md: Option<bool>,
    /// Surface a repo-root `AGENTS.md` as a top-level nav entry. Same default and
    /// rationale as `surface_claude_md`; independent of it.
    #[serde(default)]
    pub surface_agents_md: Option<bool>,
}

impl SiteConfig {
    /// Load `compositor.toml` from `project_dir`. A **missing** file is not an
    /// error — defaults are synthesized so a bare directory of Markdown can be
    /// built/served with no config at all. A file that exists but is unreadable
    /// or malformed is a hard, named error, so a typo is never silently ignored.
    pub fn load(project_dir: &Path) -> Result<SiteConfig> {
        let cfg_path = project_dir.join("compositor.toml");
        match std::fs::read_to_string(&cfg_path) {
            Ok(s) => toml::from_str(&s).with_context(|| format!("parsing {}", cfg_path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(SiteConfig::synthesized(project_dir))
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", cfg_path.display())),
        }
    }

    /// Defaults for a directory with no `compositor.toml`: the site name is the
    /// humanized folder name, and the docs live in `docs/` if that subdir
    /// exists, else the directory itself (a bare folder of Markdown).
    fn synthesized(project_dir: &Path) -> SiteConfig {
        // Canonicalize so `--dir .` resolves to the real folder name rather
        // than "" — fall back to the path as given if it can't be resolved.
        let named =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        let site_name = named
            .file_name()
            .and_then(|n| n.to_str())
            .map(humanize_filename)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Docs".to_string());
        let docs_dir = if project_dir.join("docs").is_dir() {
            "docs"
        } else {
            "."
        };
        SiteConfig {
            site_name,
            docs_dir: Some(docs_dir.to_string()),
            ..Default::default()
        }
    }

    pub fn docs_dir(&self) -> &str {
        self.docs_dir.as_deref().unwrap_or("docs")
    }

    pub fn out_dir(&self) -> &str {
        self.out_dir.as_deref().unwrap_or("site")
    }

    pub fn surface_claude_md(&self) -> bool {
        self.surface_claude_md.unwrap_or(true)
    }

    pub fn surface_agents_md(&self) -> bool {
        self.surface_agents_md.unwrap_or(true)
    }

    /// The docs directory as a path under `project_dir`. A `docs_dir` of "."
    /// resolves to `project_dir` itself (no trailing `./` component), so callers
    /// get a clean path they can prefix-compare against, e.g. the out dir.
    pub fn docs_path(&self, project_dir: &Path) -> PathBuf {
        match self.docs_dir() {
            "." => project_dir.to_path_buf(),
            d => project_dir.join(d),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("compositor-cfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn missing_toml_uses_docs_subdir_and_folder_name() {
        let tmp = scratch("sub");
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        let cfg = SiteConfig::load(&tmp).unwrap();
        assert_eq!(cfg.docs_dir(), "docs");
        assert_eq!(cfg.docs_path(&tmp), tmp.join("docs"));
        // Name derived from the folder, humanized and non-empty.
        assert!(!cfg.site_name.is_empty());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn missing_toml_bare_markdown_dir_serves_itself() {
        let tmp = scratch("bare");
        std::fs::write(tmp.join("index.md"), "# Hi").unwrap();
        let cfg = SiteConfig::load(&tmp).unwrap();
        assert_eq!(cfg.docs_dir(), ".");
        // "." resolves to the dir itself, with no "./" artifact.
        assert_eq!(cfg.docs_path(&tmp), tmp);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn present_toml_is_respected() {
        let tmp = scratch("toml");
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"Explicit\"\ndocs_dir = \"content\"\n",
        )
        .unwrap();
        let cfg = SiteConfig::load(&tmp).unwrap();
        assert_eq!(cfg.site_name, "Explicit");
        assert_eq!(cfg.docs_dir(), "content");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn malformed_toml_is_a_named_error() {
        let tmp = scratch("bad");
        std::fs::write(tmp.join("compositor.toml"), "site_name = ").unwrap();
        let err = SiteConfig::load(&tmp).unwrap_err();
        // The error names the file so the user knows what to fix.
        assert!(
            format!("{err:#}").contains("compositor.toml"),
            "err: {err:#}"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn exclude_defaults_empty_and_parses_list() {
        let tmp = scratch("excl");
        // Missing key -> empty.
        std::fs::write(tmp.join("compositor.toml"), "site_name = \"X\"\n").unwrap();
        assert!(SiteConfig::load(&tmp).unwrap().exclude.is_empty());
        // Present list -> parsed.
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\nexclude = [\"superpowers/\", \"inbox/\"]\n",
        )
        .unwrap();
        assert_eq!(
            SiteConfig::load(&tmp).unwrap().exclude,
            vec!["superpowers/".to_string(), "inbox/".to_string()]
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn surface_toggles_default_on_and_parse_off() {
        let tmp = scratch("surface");
        // No compositor.toml at all -> synthesized -> both on.
        let cfg = SiteConfig::load(&tmp).unwrap();
        assert!(cfg.surface_claude_md());
        assert!(cfg.surface_agents_md());
        // Present file, keys missing -> both on (accessor default).
        std::fs::write(tmp.join("compositor.toml"), "site_name = \"X\"\n").unwrap();
        let cfg = SiteConfig::load(&tmp).unwrap();
        assert!(cfg.surface_claude_md());
        assert!(cfg.surface_agents_md());
        // Explicit false -> off, independently.
        std::fs::write(
            tmp.join("compositor.toml"),
            "site_name = \"X\"\nsurface_claude_md = false\nsurface_agents_md = false\n",
        )
        .unwrap();
        let cfg = SiteConfig::load(&tmp).unwrap();
        assert!(!cfg.surface_claude_md());
        assert!(!cfg.surface_agents_md());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
