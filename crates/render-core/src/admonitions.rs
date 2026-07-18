//! Preprocess MkDocs/Material `!!!` and `???` admonitions into HTML wrappers.
//!
//! The blank lines around the body are load-bearing: CommonMark ends a raw-HTML
//! block at the blank line, so comrak parses the body between the wrapper tags
//! as normal Markdown in the *same* pass (link rewrite, anchors, syntect, TOC
//! all apply). Requires `render.unsafe_` on the comrak options.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    Block,         // !!!  -> <div>
    DetailsClosed, // ???  -> <details>
    DetailsOpen,   // ???+ -> <details open>
}

struct Opener {
    marker: Marker,
    classes: String,       // space-joined, e.g. "danger inline"
    title: Option<String>, // None = default; Some("") = suppress; Some(s) = literal
}

pub fn preprocess_admonitions(src: &str) -> String {
    preprocess_admonitions_mapped(src).0
}

/// Append `s` (which may itself contain internal newlines) to `out` as one or
/// more output lines, each mapped to `origin` in `map`. `s` must not carry a
/// trailing newline — callers pass line-shaped fragments only.
fn push_mapped(out: &mut String, map: &mut Vec<Option<usize>>, s: &str, origin: Option<usize>) {
    for line in s.split('\n') {
        out.push_str(line);
        out.push('\n');
        map.push(origin);
    }
}

/// Same preprocessing as [`preprocess_admonitions`], plus a per-output-line
/// map back to the source line it came from (`None` for a synthesized
/// admonition-wrapper line). The recursion for nested admonitions only needs
/// the string form — nested wrapper output is entirely synthesized from the
/// outer view regardless, so those lines are already `None` here.
pub fn preprocess_admonitions_mapped(src: &str) -> (String, Vec<Option<usize>>) {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = String::new();
    let mut map: Vec<Option<usize>> = Vec::new();
    let mut i = 0;
    let mut fence: Option<(char, usize)> = None;
    while i < lines.len() {
        let line = lines[i];
        if let Some((fc, flen)) = fence {
            push_mapped(&mut out, &mut map, line, Some(i));
            if is_fence_close(line, fc, flen) {
                fence = None;
            }
            i += 1;
            continue;
        }
        if let Some(open) = fence_open(line) {
            fence = Some(open);
            push_mapped(&mut out, &mut map, line, Some(i));
            i += 1;
            continue;
        }
        if let Some(op) = parse_opener(line) {
            let mut body_lines: Vec<String> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                let l = lines[j];
                if l.trim().is_empty() {
                    body_lines.push(String::new());
                    j += 1;
                } else if let Some(rest) = deindent4(l) {
                    body_lines.push(rest.to_string());
                    j += 1;
                } else {
                    break;
                }
            }
            while matches!(body_lines.last(), Some(s) if s.is_empty()) {
                body_lines.pop();
            }
            let body_src = body_lines.join("\n");
            let body = preprocess_admonitions(&body_src); // string-only recursion; nested output is synthesized regardless
                                                          // A leading blank line guarantees the wrapper starts a fresh HTML block.
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
                map.push(None);
            }
            let wrapper = render_wrapper(&op, body.trim_end());
            push_mapped(&mut out, &mut map, wrapper.trim_end_matches('\n'), None);
            if j < lines.len() {
                out.push('\n');
                map.push(None);
            }
            i = j;
            continue;
        }
        push_mapped(&mut out, &mut map, line, Some(i));
        i += 1;
    }
    (out, map)
}

fn deindent4(line: &str) -> Option<&str> {
    line.strip_prefix("    ")
        .or_else(|| line.strip_prefix('\t'))
}

fn fence_open(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start_matches(' ');
    for fc in ['`', '~'] {
        let count = t.chars().take_while(|&c| c == fc).count();
        if count >= 3 {
            return Some((fc, count));
        }
    }
    None
}

fn is_fence_close(line: &str, fc: char, flen: usize) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().count() >= flen && t.chars().all(|c| c == fc)
}

fn parse_opener(line: &str) -> Option<Opener> {
    let (marker, rest) = if let Some(r) = line.strip_prefix("???+") {
        (Marker::DetailsOpen, r)
    } else if let Some(r) = line.strip_prefix("???") {
        (Marker::DetailsClosed, r)
    } else {
        let r = line.strip_prefix("!!!")?;
        (Marker::Block, r)
    };
    if !rest.starts_with([' ', '\t']) {
        return None; // marker must be followed by whitespace
    }
    let rest = rest.trim();
    if rest.is_empty() {
        return None; // no type word
    }
    let (classes_part, title) = match split_title(rest) {
        Some((c, t)) => (c, Some(t)),
        None => (rest, None),
    };
    let classes: Vec<&str> = classes_part.split_whitespace().collect();
    if classes.is_empty() {
        return None;
    }
    Some(Opener {
        marker,
        classes: classes.join(" "),
        title,
    })
}

/// If `rest` ends with a quoted `"title"`, split it off. Returns (classes, title).
fn split_title(rest: &str) -> Option<(&str, String)> {
    if !rest.ends_with('"') {
        return None;
    }
    let first = rest.find('"')?;
    let last = rest.rfind('"')?;
    if first == last {
        return None; // a single stray quote is not a title
    }
    let classes = rest[..first].trim_end();
    let title = rest[first + 1..last].to_string();
    Some((classes, title))
}

fn render_wrapper(op: &Opener, body: &str) -> String {
    let (open_tag, title_tag, close_tag, open_attr) = match op.marker {
        Marker::Block => ("div", "p", "div", ""),
        Marker::DetailsClosed => ("details", "summary", "details", ""),
        Marker::DetailsOpen => ("details", "summary", "details", " open"),
    };
    let class_attr = html_escape(&format!("admonition {}", op.classes));
    let is_collapsible = matches!(op.marker, Marker::DetailsClosed | Marker::DetailsOpen);
    // Resolve the title text: None = render no title element at all. A collapsible
    // must keep a <summary> click target, so an explicitly-empty title falls back
    // to the default there (a summary-less <details> shows a browser-default label
    // and makes the whole body the toggle). A static block may suppress its bar.
    let title_text: Option<String> = match &op.title {
        Some(t) if t.is_empty() => is_collapsible.then(|| default_title(&op.classes)),
        Some(t) => Some(t.clone()),
        None => Some(default_title(&op.classes)),
    };
    let title_html = match title_text {
        Some(t) => format!(
            "<{title_tag} class=\"admonition-title\">{}</{title_tag}>\n",
            html_escape(&t)
        ),
        None => String::new(),
    };
    let mut s = String::new();
    s.push_str(&format!("<{open_tag} class=\"{class_attr}\"{open_attr}>\n"));
    s.push_str(&title_html);
    s.push('\n'); // blank line ends the raw-HTML block; body renders as Markdown
    s.push_str(body);
    s.push_str("\n\n"); // blank line before the closing tag
    s.push_str(&format!("</{close_tag}>\n"));
    s
}

fn default_title(classes: &str) -> String {
    let first = classes.split_whitespace().next().unwrap_or("");
    let mut c = first.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_block_default_title() {
        let out = preprocess_admonitions("!!! note\n    Body text.\n");
        assert!(out.contains("<div class=\"admonition note\">"), "{out}");
        assert!(
            out.contains("<p class=\"admonition-title\">Note</p>"),
            "{out}"
        );
        assert!(out.contains("Body text."), "{out}");
        assert!(out.contains("</div>"), "{out}");
    }

    #[test]
    fn custom_title_used_verbatim() {
        let out = preprocess_admonitions("!!! warning \"Be careful\"\n    Body.\n");
        assert!(out.contains("<div class=\"admonition warning\">"), "{out}");
        assert!(
            out.contains("<p class=\"admonition-title\">Be careful</p>"),
            "{out}"
        );
    }

    #[test]
    fn empty_title_suppresses_title_element() {
        let out = preprocess_admonitions("!!! note \"\"\n    Body.\n");
        assert!(out.contains("<div class=\"admonition note\">"), "{out}");
        assert!(!out.contains("admonition-title"), "{out}");
    }

    #[test]
    fn multiple_classes_passed_through() {
        let out = preprocess_admonitions("!!! danger inline\n    Body.\n");
        assert!(
            out.contains("<div class=\"admonition danger inline\">"),
            "{out}"
        );
        // Default title is the first class word, capitalized.
        assert!(out.contains(">Danger</p>"), "{out}");
    }

    #[test]
    fn body_is_deindented_by_four() {
        let out = preprocess_admonitions("!!! note\n        code-ish line\n");
        // 8 leading spaces -> 4 remain after de-indent (so nested markdown keeps its shape).
        assert!(out.contains("\n    code-ish line"), "{out}");
    }

    #[test]
    fn nested_admonition_recurses() {
        let src =
            "!!! note \"Outer\"\n    Outer body.\n\n    !!! tip \"Inner\"\n        Inner body.\n";
        let out = preprocess_admonitions(src);
        assert!(out.contains("<div class=\"admonition note\">"), "{out}");
        assert!(out.contains("<div class=\"admonition tip\">"), "{out}");
        assert!(out.contains(">Inner</p>"), "{out}");
    }

    #[test]
    fn opener_inside_code_fence_is_untouched() {
        let src = "```\n!!! note\n    body here\n```\n";
        let out = preprocess_admonitions(src);
        assert!(!out.contains("class=\"admonition"), "{out}");
        assert!(out.contains("!!! note"), "{out}");
    }

    #[test]
    fn malformed_opener_passes_through() {
        // `!!!` with no type is not a valid opener.
        let out = preprocess_admonitions("!!!\n    Body.\n");
        assert!(!out.contains("<div"), "{out}");
        assert!(out.contains("!!!"), "{out}");
    }

    #[test]
    fn title_only_no_body() {
        let out = preprocess_admonitions("!!! note\nnot indented, so not body\n");
        assert!(out.contains("<div class=\"admonition note\">"), "{out}");
        // The non-indented following line stays outside the admonition.
        assert!(out.contains("not indented, so not body"), "{out}");
    }

    #[test]
    fn blank_line_separates_admonition_from_following_content() {
        let out = preprocess_admonitions("!!! note\n    Body.\n\nNext paragraph.\n");
        // A blank line must follow the closing tag so downstream Markdown parsing
        // treats the following text as its own block, not raw HTML block content.
        assert!(out.contains("</div>\n\nNext paragraph."), "{out}");
    }

    #[test]
    fn escapes_html_in_title_and_class() {
        let out = preprocess_admonitions("!!! note <danger&x> \"a <b> & \\\"c\\\"\"\n    Body.\n");
        // Title special chars escaped.
        assert!(out.contains("&lt;b&gt;"), "{out}");
        assert!(out.contains("&amp;"), "{out}");
        // Class word special chars escaped in the class attribute too.
        assert!(out.contains("&lt;danger&amp;x&gt;"), "{out}");
    }

    #[test]
    fn collapsible_closed_default_title() {
        let out = preprocess_admonitions("??? note\n    Body text.\n");
        assert!(out.contains("<details class=\"admonition note\">"), "{out}");
        // The closed variant must not carry the `open` attribute on its tag.
        assert!(!out.contains("admonition note\" open>"), "{out}");
        assert!(
            out.contains("<summary class=\"admonition-title\">Note</summary>"),
            "{out}"
        );
        assert!(out.contains("Body text."), "{out}");
        assert!(out.contains("</details>"), "{out}");
    }

    #[test]
    fn collapsible_open_default_title() {
        let out = preprocess_admonitions("???+ note\n    Body text.\n");
        assert!(
            out.contains("<details class=\"admonition note\" open>"),
            "{out}"
        );
        assert!(
            out.contains("<summary class=\"admonition-title\">Note</summary>"),
            "{out}"
        );
        assert!(out.contains("Body text."), "{out}");
        assert!(out.contains("</details>"), "{out}");
    }

    #[test]
    fn collapsible_with_custom_title() {
        let src_closed = "??? warning \"Be careful\"\n    Body.\n";
        let out_closed = preprocess_admonitions(src_closed);
        assert!(
            out_closed.contains("<details class=\"admonition warning\">"),
            "{out_closed}"
        );
        assert!(
            out_closed.contains("<summary class=\"admonition-title\">Be careful</summary>"),
            "{out_closed}"
        );

        let src_open = "???+ tip \"Pro tip\"\n    Body.\n";
        let out_open = preprocess_admonitions(src_open);
        assert!(
            out_open.contains("<details class=\"admonition tip\" open>"),
            "{out_open}"
        );
        assert!(
            out_open.contains("<summary class=\"admonition-title\">Pro tip</summary>"),
            "{out_open}"
        );
    }

    #[test]
    fn collapsible_empty_title_falls_back_to_default_summary() {
        // A collapsible needs a clickable <summary>; an explicitly-empty title
        // must not drop it (a summary-less <details> shows a browser-default
        // label). The block variant still suppresses its title bar (see
        // `empty_title_suppresses_title_element`).
        let out = preprocess_admonitions("??? note \"\"\n    Body.\n");
        assert!(out.contains("<details class=\"admonition note\">"), "{out}");
        assert!(
            out.contains("<summary class=\"admonition-title\">Note</summary>"),
            "{out}"
        );
    }

    #[test]
    fn line_map_is_identity_without_admonitions() {
        let (out, map) = preprocess_admonitions_mapped("# A\n\npara\n\n- x\n");
        assert_eq!(out, "# A\n\npara\n\n- x\n");
        // Every output line maps back to the same source line.
        assert_eq!(map, vec![Some(0), Some(1), Some(2), Some(3), Some(4)]);
    }

    #[test]
    fn line_map_marks_admonition_lines_none_and_keeps_trailing_mapping() {
        let src = "intro\n\n!!! note\n    body\n\nafter\n";
        let (_out, map) = preprocess_admonitions_mapped(src);
        // "intro" maps to source line 0.
        assert_eq!(map[0], Some(0));
        // The admonition-wrapper output lines are synthesized.
        assert!(map.iter().any(|m| m.is_none()));
        // "after" (source line 5) is still recoverable somewhere in the map: no
        // passthrough line after the admonition may map to the wrong source line.
        assert!(map.contains(&Some(5)));
        // No passthrough line ever points past the last real source line.
        assert!(map.iter().flatten().all(|&s| s <= 5));
    }
}
