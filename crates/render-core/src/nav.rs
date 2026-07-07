use std::collections::BTreeMap;
use crate::site::{humanize_filename, Page};

pub enum NavNode {
    Page { title: String, url: String },
    Section { title: String, children: Vec<NavNode> },
}

pub struct NavTree(pub Vec<NavNode>);

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
        let stem = file[0].trim_end_matches(".md");
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
        out.push(NavNode::Page { title: title.clone(), url: url.clone() });
    }
    for (name, sub) in dir.subdirs.iter_mut() {
        out.push(NavNode::Section {
            title: humanize_filename(name),
            children: render_dir(sub),
        });
    }
    out
}
