use render_core::nav::{NavNode, tree_from_pages};
use render_core::site::Page;
use std::path::PathBuf;

fn page(rel: &str, title: &str) -> Page {
    Page {
        rel_path: PathBuf::from(rel),
        url: rel.replace(".md", ".html"),
        title: title.into(),
        html: String::new(),
    }
}

#[test]
fn sections_from_dirs_index_first_then_alpha() {
    let pages = vec![
        page("index.md", "Home"),
        page("cli/tar.md", "Tar"),
        page("cli/index.md", "CLI Home"),
        page("cli/bash.md", "Bash"),
    ];
    let tree = tree_from_pages(&pages);

    // Top level: Home page, then "Cli" section.
    match &tree.0[0] {
        NavNode::Page { title, url } => {
            assert_eq!(title, "Home");
            assert_eq!(url, "index.html");
        }
        _ => panic!("expected top page first"),
    }
    match &tree.0[1] {
        NavNode::Section { title, children } => {
            assert_eq!(title, "Cli");
            // index first, then alpha: CLI Home, Bash, Tar
            let titles: Vec<_> = children.iter().map(|c| match c {
                NavNode::Page { title, .. } => title.clone(),
                _ => "SECTION".into(),
            }).collect();
            assert_eq!(titles, vec!["CLI Home", "Bash", "Tar"]);
        }
        _ => panic!("expected section second"),
    }
}

#[test]
fn sections_and_pages_order_case_insensitively() {
    let pages = vec![
        // Sibling sections: "Zoo/" and "apple/" -> titles "Zoo", "Apple".
        // Case-sensitive (ASCII) order would put "Zoo" before "apple"
        // since 'Z' (90) < 'a' (97); case-insensitive order must put
        // "Apple" first.
        page("Zoo/index.md", "Zoo Home"),
        page("apple/index.md", "Apple Home"),
        // Page-level: a lowercase-leading title ("bike") must still sort
        // before an uppercase-leading one ("Car") case-insensitively,
        // even though ASCII order would reverse them ('C' 67 < 'b' 98).
        page("apple/car.md", "Car"),
        page("apple/bike.md", "bike"),
    ];
    let tree = tree_from_pages(&pages);

    // Top level: two sections, "Apple" then "Zoo" (case-insensitive alpha).
    let section_titles: Vec<_> = tree.0.iter().map(|n| match n {
        NavNode::Section { title, .. } => title.clone(),
        NavNode::Page { title, .. } => title.clone(),
    }).collect();
    assert_eq!(section_titles, vec!["Apple", "Zoo"]);

    match &tree.0[0] {
        NavNode::Section { title, children } => {
            assert_eq!(title, "Apple");
            // index first, then alpha case-insensitive: bike, Car.
            let titles: Vec<_> = children.iter().map(|c| match c {
                NavNode::Page { title, .. } => title.clone(),
                _ => "SECTION".into(),
            }).collect();
            assert_eq!(titles, vec!["Apple Home", "bike", "Car"]);
        }
        _ => panic!("expected Apple section first"),
    }
}

#[test]
fn section_order_requires_case_folding_not_just_humanized_title() {
    // "Ab" and "aB" are case-insensitive duplicates of the same word, so
    // humanize_filename alone (which only forces the *first* character of
    // a title to uppercase) is not enough to make their titles compare
    // equal without folding: humanize_filename("Ab") == "Ab" and
    // humanize_filename("aB") == "AB", and "Ab" vs "AB" compare
    // differently under raw ASCII ordering than under case folding.
    // Only comparing `.to_lowercase()` of the humanized titles correctly
    // treats them as equal and falls through to a deterministic
    // (raw-name) tie-break, which orders "Ab" before "aB".
    let pages = vec![
        page("aB/index.md", "aB Home"),
        page("Ab/index.md", "Ab Home"),
    ];
    let tree = tree_from_pages(&pages);

    let section_titles: Vec<_> = tree.0.iter().map(|n| match n {
        NavNode::Section { title, .. } => title.clone(),
        NavNode::Page { title, .. } => title.clone(),
    }).collect();
    assert_eq!(section_titles, vec!["Ab", "AB"]);
}
