use serde::Deserialize;

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
}

impl SiteConfig {
    pub fn docs_dir(&self) -> &str { self.docs_dir.as_deref().unwrap_or("docs") }
    pub fn out_dir(&self) -> &str { self.out_dir.as_deref().unwrap_or("site") }
}
