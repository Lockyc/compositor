use crate::site::{humanize_filename, Page};
use std::collections::BTreeMap;

pub enum NavNode {
    Page {
        title: String,
        url: String,
    },
    Section {
        title: String,
        children: Vec<NavNode>,
    },
}

pub struct NavTree(pub Vec<NavNode>);

/// One page in linear reading order — the flattened nav, used to derive
/// prev/next page links.
pub struct NavLink {
    pub title: String,
    pub url: String,
}

/// Flatten the nav tree into linear reading order: pages in the order they
/// appear (index first, then alphabetical), descending into each section in
/// place — the exact order the sidebar menu renders. Sections themselves are
/// not links, only their pages are.
pub fn flatten(nav: &NavTree) -> Vec<NavLink> {
    let mut out = Vec::new();
    collect(&nav.0, &mut out);
    out
}

fn collect(nodes: &[NavNode], out: &mut Vec<NavLink>) {
    for node in nodes {
        match node {
            NavNode::Page { title, url } => out.push(NavLink {
                title: title.clone(),
                url: url.clone(),
            }),
            NavNode::Section { children, .. } => collect(children, out),
        }
    }
}

// Intermediate mutable tree keyed by path component.
#[derive(Default)]
struct Dir {
    subdirs: BTreeMap<String, Dir>,
    files: Vec<(String, String, String)>, // (sort_key, title, url)
}

pub fn tree_from_pages(pages: &[Page]) -> NavTree {
    let mut root = Dir::default();
    for p in pages {
        let comps: Vec<String> = p
            .rel_path
            .iter()
            .map(|c| c.to_string_lossy().to_string())
            .collect();
        let (dirs, file) = comps.split_at(comps.len() - 1);
        let mut cur = &mut root;
        for d in dirs {
            cur = cur.subdirs.entry(d.clone()).or_default();
        }
        let stem = file[0].strip_suffix(".md").unwrap_or(&file[0]);
        // index sorts before everything: prefix key with '0', else '1'.
        let sort_key = if stem == "index" {
            "0".to_string()
        } else {
            format!("1{}", p.title.to_lowercase())
        };
        cur.files.push((sort_key, p.title.clone(), p.url.clone()));
    }
    NavTree(render_dir(&mut root))
}

fn render_dir(dir: &mut Dir) -> Vec<NavNode> {
    let mut out = Vec::new();
    dir.files.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, title, url) in &dir.files {
        out.push(NavNode::Page {
            title: title.clone(),
            url: url.clone(),
        });
    }
    // Sections: alphabetical by title, case-insensitive (tie-break by raw
    // name for determinism when titles collide case-insensitively).
    let mut subdirs: Vec<(&String, &mut Dir)> = dir.subdirs.iter_mut().collect();
    subdirs.sort_by(|(a_name, _), (b_name, _)| {
        let a_key = humanize_filename(a_name).to_lowercase();
        let b_key = humanize_filename(b_name).to_lowercase();
        a_key.cmp(&b_key).then_with(|| a_name.cmp(b_name))
    });
    for (name, sub) in subdirs {
        out.push(NavNode::Section {
            title: humanize_filename(name),
            children: render_dir(sub),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_walks_reading_order_pages_then_sections() {
        let nav = NavTree(vec![
            NavNode::Page {
                title: "Home".into(),
                url: "index.html".into(),
            },
            NavNode::Page {
                title: "Guide".into(),
                url: "guide.html".into(),
            },
            NavNode::Section {
                title: "Topics".into(),
                children: vec![
                    NavNode::Page {
                        title: "Alpha".into(),
                        url: "topics/alpha.html".into(),
                    },
                    NavNode::Page {
                        title: "Beta".into(),
                        url: "topics/beta.html".into(),
                    },
                ],
            },
        ]);
        let urls: Vec<String> = flatten(&nav).into_iter().map(|l| l.url).collect();
        assert_eq!(
            urls,
            [
                "index.html",
                "guide.html",
                "topics/alpha.html",
                "topics/beta.html"
            ]
        );
    }
}
