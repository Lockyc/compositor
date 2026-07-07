use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct FrontMatter {
    #[serde(default)]
    pub title: Option<String>,
}

pub fn split_frontmatter(input: &str) -> (FrontMatter, String) {
    // Frontmatter must start on the very first line as `---`.
    let rest = match input.strip_prefix("---\n") {
        Some(r) => r,
        None => return (FrontMatter::default(), input.to_string()),
    };
    // Find the closing delimiter line.
    let Some(end) = rest.find("\n---\n") else {
        return (FrontMatter::default(), input.to_string());
    };
    let yaml = &rest[..end];
    let body = &rest[end + "\n---\n".len()..];
    let fm = serde_yaml::from_str::<FrontMatter>(yaml).unwrap_or_default();
    (fm, body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_frontmatter_returns_body_unchanged() {
        let (fm, body) = split_frontmatter("# Hello\n\ntext");
        assert!(fm.title.is_none());
        assert_eq!(body, "# Hello\n\ntext");
    }

    #[test]
    fn extracts_title_and_strips_block() {
        let input = "---\ntitle: My Page\ntype: entity\n---\n# Body\n";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm.title.as_deref(), Some("My Page"));
        assert_eq!(body, "# Body\n");
    }

    #[test]
    fn unknown_keys_are_ignored_not_errors() {
        let input = "---\ntags: [a, b]\nconfidence: high\n---\nbody";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.title.is_none());
        assert_eq!(body, "body");
    }

    #[test]
    fn malformed_yaml_falls_back_to_default() {
        let input = "---\ntitle: : :\n---\nbody";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.title.is_none());
        assert_eq!(body, "body");
    }
}
