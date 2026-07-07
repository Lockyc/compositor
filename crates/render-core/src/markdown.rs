use comrak::{parse_document, format_html_with_plugins, Arena, Options, Plugins};
use comrak::plugins::syntect::SyntectAdapter;
use comrak::nodes::{AstNode, NodeValue};

pub struct Rendered {
    pub html: String,
    pub first_h1: Option<String>,
}

pub fn comrak_options<'c>() -> Options<'c> {
    let mut o = Options::default();
    o.extension.table = true;
    o.extension.tasklist = true;
    o.extension.strikethrough = true;
    o.extension.autolink = true;
    o.extension.header_ids = Some(String::new()); // heading anchors
    o
}

pub fn render_markdown(body: &str) -> Rendered {
    let arena = Arena::new();
    let options = comrak_options();
    let root = parse_document(&arena, body, &options);

    let first_h1 = find_first_h1(root);

    let adapter = SyntectAdapter::new(Some("InspiredGitHub"));
    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&adapter);

    let mut out = Vec::new();
    format_html_with_plugins(root, &options, &mut out, &plugins)
        .expect("comrak html formatting is infallible for in-memory writer");
    Rendered {
        html: String::from_utf8(out).expect("comrak emits valid utf-8"),
        first_h1,
    }
}

fn find_first_h1<'a>(node: &'a AstNode<'a>) -> Option<String> {
    for child in node.descendants() {
        if let NodeValue::Heading(h) = &child.data.borrow().value {
            if h.level == 1 {
                return Some(text_of(child));
            }
        }
    }
    None
}

fn text_of<'a>(node: &'a AstNode<'a>) -> String {
    let mut s = String::new();
    for d in node.descendants() {
        if let NodeValue::Text(t) = &d.data.borrow().value {
            s.push_str(t);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_gfm_table() {
        let r = render_markdown("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(r.html.contains("<table>"));
    }

    #[test]
    fn highlights_fenced_code() {
        let r = render_markdown("```rust\nfn main() {}\n```");
        // syntect emits inline styles on a <pre>/<code> span structure
        assert!(r.html.contains("style=\"") && r.html.contains("main"));
    }

    #[test]
    fn extracts_first_h1() {
        let r = render_markdown("# Title Here\n\nbody");
        assert_eq!(r.first_h1.as_deref(), Some("Title Here"));
    }

    #[test]
    fn no_h1_is_none() {
        let r = render_markdown("## Sub only\n\nbody");
        assert!(r.first_h1.is_none());
    }
}
