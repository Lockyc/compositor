use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct FrontMatter {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

pub fn split_frontmatter(input: &str) -> (FrontMatter, String) {
    // Frontmatter must start on the very first line as `---`, with LF or CRLF
    // endings (a Windows-authored or autocrlf file). `body` stays a true suffix
    // of `input`, which the editor's frontmatter line count depends on.
    let (rest, close) = if let Some(r) = input.strip_prefix("---\n") {
        (r, "\n---\n")
    } else if let Some(r) = input.strip_prefix("---\r\n") {
        (r, "\r\n---\r\n")
    } else {
        return (FrontMatter::default(), input.to_string());
    };
    // Find the closing delimiter line.
    let Some(end) = rest.find(close) else {
        return (FrontMatter::default(), input.to_string());
    };
    let yaml = &rest[..end];
    let body = &rest[end + close.len()..];
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
    fn crlf_frontmatter_is_recognised() {
        let input = "---\r\ntitle: Win\r\naliases: [w]\r\n---\r\n# Body\r\n";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm.title.as_deref(), Some("Win"));
        assert_eq!(fm.aliases, vec!["w".to_string()]);
        assert_eq!(body, "# Body\r\n");
        assert!(input.ends_with(&body));
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

    #[test]
    fn extracts_aliases_list() {
        let input = "---\ntitle: Setup Guide\naliases: [setup, install]\n---\nbody";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm.title.as_deref(), Some("Setup Guide"));
        assert_eq!(fm.aliases, vec!["setup".to_string(), "install".to_string()]);
        assert_eq!(body, "body");
    }

    #[test]
    fn absent_aliases_default_to_empty() {
        let (fm, _) = split_frontmatter("---\ntitle: X\n---\nbody");
        assert!(fm.aliases.is_empty());
    }
}
