use render_core::markdown::{render_markdown, LinkPolicy};
use std::collections::HashSet;
use std::path::Path;

fn urls(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn rewrites_relative_md_link_to_html() {
    let known = urls(&["cli/tar.html"]);
    let r = render_markdown(
        "[tar](tar.md)",
        Path::new("cli"),
        &known,
        LinkPolicy::Strict,
    )
    .unwrap();
    assert!(r.html.contains("href=\"tar.html\""));
}

#[test]
fn leaves_external_links_untouched() {
    let known = urls(&[]);
    let r = render_markdown(
        "[x](https://example.com)",
        Path::new(""),
        &known,
        LinkPolicy::Strict,
    )
    .unwrap();
    assert!(r.html.contains("href=\"https://example.com\""));
}

#[test]
fn errors_on_unresolvable_internal_link() {
    let known = urls(&["cli/tar.html"]);
    let err = render_markdown(
        "[gone](missing.md)",
        Path::new("cli"),
        &known,
        LinkPolicy::Strict,
    );
    assert!(err.is_err());
}

#[test]
fn lenient_policy_renders_unresolvable_link_as_broken_html() {
    let known = urls(&["cli/tar.html"]);
    // Lenient: no error, and the dead link is still rewritten .md -> .html
    // (an honest 404 target) rather than aborting the whole render.
    let r = render_markdown(
        "[gone](missing.md)",
        Path::new("cli"),
        &known,
        LinkPolicy::Lenient,
    )
    .unwrap();
    assert!(r.html.contains("href=\"missing.html\""));
}

#[test]
fn rewrites_dotdot_relative_link_using_original_relative_path() {
    // known_urls uses the normalized/joined form ("cli/other.html"), but the
    // emitted href must stay relative to the page ("../other.html"), not the
    // normalized/joined path. This distinguishes emit-relative (correct) from
    // emit-normalized (the prior buggy behavior).
    let known = urls(&["cli/other.html"]);
    let r = render_markdown(
        "[o](../other.md)",
        Path::new("cli/sub"),
        &known,
        LinkPolicy::Strict,
    )
    .unwrap();
    assert!(r.html.contains("href=\"../other.html\""));
    assert!(!r.html.contains("href=\"cli/other.html\""));
}

#[test]
fn only_the_trailing_md_extension_is_swapped_not_earlier_occurrences() {
    // A filename that itself contains ".md" before the real extension. url_for
    // strips only the trailing ".md" (-> "notes.md.html"), so link rewriting
    // must do the same: an all-occurrences replace would compute
    // "notes.html.html", mismatch the known url, and wrongly error the link as
    // unresolvable — and even if resolved, emit a corrupted href.
    let known = urls(&["notes.md.html"]);
    let r = render_markdown(
        "[n](notes.md.md)",
        Path::new(""),
        &known,
        LinkPolicy::Strict,
    )
    .unwrap();
    assert!(r.html.contains("href=\"notes.md.html\""));
}

#[test]
fn preserves_anchor_fragment_on_rewritten_link() {
    let known = urls(&["cli/tar.html"]);
    let r = render_markdown(
        "[s](tar.md#sec)",
        Path::new("cli"),
        &known,
        LinkPolicy::Strict,
    )
    .unwrap();
    assert!(r.html.contains("href=\"tar.html#sec\""));
}
