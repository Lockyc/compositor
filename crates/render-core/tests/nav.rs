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
