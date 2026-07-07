# compositor

## Purpose

A Rust static-site generator for Markdown doc repos, replacing MkDocs across the
the docs sites. `compositor build <dir>` renders a directory of Markdown into a themed,
tree-navigated, Pagefind-searchable static HTML site.

## Current state

Milestone 1 (plain-GFM `build`) in progress. This commit is the initial workspace
scaffold: a compiling two-crate workspace with a temporary smoke symbol
(`render_core::hello()`), no real rendering yet.

## Layout

Cargo workspace, two crates:

```
Cargo.toml                      # [workspace] members
crates/
  render-core/                  # library: Markdown -> HTML, frontmatter, title
                                 # resolution, nav tree, link rewrite -> SiteModel.
                                 # No CLI/disk assumptions, so `serve` and a future
                                 # Tauri app can reuse it.
  compositor/                   # CLI crate: config load, theme wrap (askama),
                                 # write out_dir, invoke Pagefind.
```

## Milestone-1 render surface (exhaustive)

- GFM: headings, lists, tables, task lists, autolinks, strikethrough, images,
  blockquotes.
- Fenced code with syntect highlighting.
- Frontmatter `title` key only (consume; ignore all other keys).
- Internal `.md` -> `.html` link rewrite.
- Heading anchors + per-page TOC.
- Tree-derived nav (directories become sections, alphabetical, `index.md` first).
- Title resolution: `frontmatter.title` -> first `# H1` -> humanized filename.

Explicitly **not** in Milestone 1 (later plans): `[[wikilinks]]`, `!!!`
admonitions, explicit-`nav` config override, the `serve` dev server, host
rollout. No functionality duplicated from `docgate`: `build` fails only on an
unresolvable internal link (a render error) — orphan/graph auditing stays
docgate's.

## Commands

- Build: `cargo build`
- Test: `cargo test`
