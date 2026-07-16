# compositor

A Rust static-site generator for Markdown doc repos: point it at a directory of
Markdown and get a themed, navigable, search-indexed static site. Built as a
from-scratch replacement for MkDocs across a fleet of documentation sites, and as a
reusable render engine (`render-core`) that lector — a Tauri desktop docs console —
embeds.

[![CI](https://github.com/Lockyc/compositor/actions/workflows/ci.yml/badge.svg)](https://github.com/Lockyc/compositor/actions/workflows/ci.yml)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-555)
![Rust](https://img.shields.io/badge/rust-stable-orange?logo=rust&logoColor=white)
[![License](https://img.shields.io/github/license/Lockyc/compositor)](LICENSE)

## Status

**Work in progress.** Milestone 1 (`compositor build`) is complete: plain-GFM
Markdown → themed, tree-navigated, Pagefind-indexed static HTML, with non-Markdown
assets copied through, verified end to end against a real 42-page docs site.
Milestone 4 (`compositor serve`) is also complete: a live-reload dev server that
watches the docs tree, rebuilds in memory on every change, and refreshes every
open browser tab automatically. The theme-polish pass has also landed: a
Pico.css-based shell with a top bar (brand, Pagefind search box, light/dark
toggle), a left tree-nav with active-page highlighting, and a server-side
per-page TOC with scroll-spy. Search is live in `build` output; it's unavailable
under `serve` (which renders in memory and never runs Pagefind). MkDocs-style `!!!`
admonitions (and `???` collapsibles) now render too; raw HTML in your Markdown
renders as-is (matching MkDocs — compositor assumes author-trusted content).
`[[wikilinks]]` also resolve pages by title, filename, alias, or path, with the
page's resolved title (frontmatter `title` → first `# H1` → humanized filename)
as link text. See [`CLAUDE.md`](CLAUDE.md) for the full render surface and roadmap.

## Build & use

```sh
cargo build --release
./target/release/compositor build --dir path/to/docs-repo
```

A `compositor.toml` is optional. With one, it sets `site_name` (optionally
`site_url`, `repo_url`, `docs_dir` [default `docs`], `out_dir` [default `site`],
`exclude` [default: none]). Without one, defaults are synthesized: `site_name`
from the folder name, and the docs are taken from `docs/` if that subdir exists,
else the directory itself — so a bare folder of Markdown builds and serves with
no config. (A `compositor.toml` that exists but is malformed is a hard error, not
a silent fallback.) `exclude` is a list of docs-dir-relative path prefixes (e.g.
`["superpowers/"]`) skipped in rendering and asset-copy, and honored by `serve`'s
on-demand asset serving too. The rendered site lands in `out_dir`; if the
`pagefind` binary is on PATH it is invoked automatically to build the search
index.

By default `build` is strict: an unresolvable internal link fails the build.
Pass `--lenient` to publish anyway — the broken link renders as an honest 404 —
for unattended pipelines that must never miss an update over one bad link.

For local editing, `serve` watches the docs tree and live-reloads the browser on
every change:

```sh
./target/release/compositor serve --dir path/to/docs-repo --open
```

(`--host` defaults to `127.0.0.1`; omit `--port` to let the OS pick a free port —
the bound URL is printed on start — or pass `--port` to pin one. `--open` launches
the default browser.)

## Development

```sh
just build    # cargo build
just test     # cargo test
just gate     # fmt-check + clippy + test (the pre-merge gate)
```

Branch model and release process live in [`CLAUDE.md`](CLAUDE.md).

## License

MIT — see [`LICENSE`](LICENSE).
