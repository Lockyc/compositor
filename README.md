# compositor

A Rust static-site generator for Markdown doc repos: point it at a directory of
Markdown and get a themed, navigable, search-indexed static site. Built as a
from-scratch replacement for MkDocs across the the docs documentation sites, and as a
reusable render engine (`render-core`) that a future desktop viewer can embed.

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
open browser tab automatically. Not yet built: `!!!` admonitions, `[[wikilinks]]`,
and the theme-polish pass — which adds a per-page TOC and the Pagefind **search
box** (the index is built now; the UI is not). See [`CLAUDE.md`](CLAUDE.md) for
the full render surface and roadmap.

## Build & use

```sh
cargo build --release
./target/release/compositor build --dir path/to/docs-repo
```

A `compositor.toml` is optional. With one, it sets `site_name` (optionally
`site_url`, `repo_url`, `docs_dir` [default `docs`], `out_dir` [default `site`]).
Without one, defaults are synthesized: `site_name` from the folder name, and the
docs are taken from `docs/` if that subdir exists, else the directory itself — so
a bare folder of Markdown builds and serves with no config. (A `compositor.toml`
that exists but is malformed is a hard error, not a silent fallback.) The rendered
site lands in `out_dir`; if the `pagefind` binary is on PATH it is invoked
automatically to build the search index.

For local editing, `serve` watches the docs tree and live-reloads the browser on
every change:

```sh
./target/release/compositor serve --dir path/to/docs-repo --open
```

(`--host` and `--port` default to `127.0.0.1:8000`; `--open` launches the default
browser.)

## Development

```sh
just build    # cargo build
just test     # cargo test
just gate     # fmt-check + clippy + test (the pre-merge gate)
```

Branch model and release process live in [`CLAUDE.md`](CLAUDE.md).

## License

MIT — see [`LICENSE`](LICENSE).
