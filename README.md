# compositor

A Rust static-site generator for Markdown doc repos: point it at a directory of
Markdown and get a themed, navigable, searchable static site. Built as a
from-scratch replacement for MkDocs across the the docs documentation sites, and as a
reusable render engine (`render-core`) that a future desktop viewer can embed.

[![CI](https://github.com/Lockyc/compositor/actions/workflows/ci.yml/badge.svg)](https://github.com/Lockyc/compositor/actions/workflows/ci.yml)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-555)
![Rust](https://img.shields.io/badge/rust-stable-orange?logo=rust&logoColor=white)
[![License](https://img.shields.io/github/license/Lockyc/compositor)](LICENSE)

## Status

**Work in progress.** Milestone 1 (`compositor build`) is complete: plain-GFM
Markdown → themed, tree-navigated, Pagefind-indexed static HTML, verified end to end
against a real 42-page docs site. Not yet built: `!!!` admonitions, `[[wikilinks]]`,
a `serve` dev server, and the theme-polish pass (including a per-page TOC). See
[`CLAUDE.md`](CLAUDE.md) for the full render surface and roadmap.

## Build & use

```sh
cargo build --release
./target/release/compositor build --dir path/to/docs-repo
```

The project directory needs a `compositor.toml` setting `site_name` (optionally
`site_url`, `repo_url`, `docs_dir` [default `docs`], `out_dir` [default `site`]).
The rendered site lands in `out_dir`; if the `pagefind` binary is on PATH it is
invoked automatically to build the search index.

## Development

```sh
just build    # cargo build
just test     # cargo test
just gate     # fmt-check + clippy + test (the pre-merge gate)
```

Branch model and release process live in [`CLAUDE.md`](CLAUDE.md).

## License

MIT — see [`LICENSE`](LICENSE).
