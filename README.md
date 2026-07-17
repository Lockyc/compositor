# compositor

A Rust static-site generator for Markdown doc repos: point it at a directory of
Markdown and get a themed, navigable static site. Built as a
from-scratch replacement for MkDocs across a fleet of documentation sites, and as a
reusable render engine (`render-core`) that lector — a Tauri desktop docs console —
embeds.

[![Release](https://img.shields.io/github/v/release/Lockyc/compositor?sort=semver&label=release)](https://github.com/Lockyc/compositor/releases/latest)
[![CI](https://github.com/Lockyc/compositor/actions/workflows/ci.yml/badge.svg)](https://github.com/Lockyc/compositor/actions/workflows/ci.yml)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-555)
[![Rust](https://img.shields.io/badge/rust-pinned%20(rust--toolchain.toml)-orange?logo=rust&logoColor=white)](rust-toolchain.toml)
[![License](https://img.shields.io/github/license/Lockyc/compositor)](LICENSE)

## Status

**In use, and still `0.x`.** compositor has replaced MkDocs on every site it was
built for — it builds them all today, and the base-config repo it superseded is
gone. It stays `0.x` because the CLI and `compositor.toml` surface may still
move; the rendering is the settled part.

**Two commands.** `build` renders a docs tree to static HTML. `serve` is a
live-reload dev server: it watches the tree, rebuilds in memory on change, and
refreshes open browser tabs.

**What you get, with no config:** a Pico.css shell (top bar, light/dark toggle
that persists, a tree-nav marking the active page, a per-page TOC with
scroll-spy, a prev/next pager, a footer); GFM with syntect-highlighted code;
MkDocs-style `!!!` admonitions and `???` collapsibles; `[[wikilinks]]` resolved
by title, filename, alias, or path; frontmatter `title`/`aliases`; `.md`→`.html`
link rewriting; and non-Markdown assets copied through verbatim. Images resolve
against the page that references them — including a repo-root `README.md` or
`CLAUDE.md` surfaced into the site, whose images resolve against the repo root
and are copied in on reference. Raw HTML in your Markdown renders as-is,
matching MkDocs — compositor assumes author-trusted content.

**There is no site search**, deliberately — see [`CLAUDE.md`](CLAUDE.md).

**As a library**, the `render-core` crate turns a Markdown tree into an in-memory
site model, and compositor's `[lib]` target exposes `serve_handle`/`ServeHandle`
— an embedding API that runs a site on a loopback port, for host apps supervising
many sites in one process. lector, a Tauri desktop docs console, is the consumer
it was built for.

See [`CLAUDE.md`](CLAUDE.md) for the full render surface and the known
divergences from MkDocs.

## Build & use

```sh
cargo build --release
./target/release/compositor build --dir path/to/docs-repo
```

That works anywhere Rust does — compositor is developed on macOS and runs on Linux.

Each [release](https://github.com/Lockyc/compositor/releases/latest) also ships a
prebuilt `x86_64-unknown-linux-gnu` binary and its `.sha256`: a single binary with
no runtime dependencies, for dropping onto a Linux box that has no Rust toolchain
(which is exactly what it's there for). Linux is the only prebuilt target —
everywhere else, build from source with the two lines above.

A `compositor.toml` is optional. With one, it sets `site_name` (optionally
`site_url`, `repo_url`, `docs_dir` [default `docs`], `out_dir` [default `site`],
`exclude` [default: none]). Without one, defaults are synthesized: `site_name`
from the folder name, and the docs are taken from `docs/` if that subdir exists,
else the directory itself — so a bare folder of Markdown builds and serves with
no config. (A `compositor.toml` that exists but is malformed is a hard error, not
a silent fallback.)

Paths your repo's `.gitignore` ignores are skipped — in rendering, asset-copy, and
`serve`'s on-demand asset serving. Untracked scratch isn't site content, so no
config is needed to keep it out, and a directory that isn't a git repo ignores
nothing. Repo `.gitignore` files only: the global `~/.config/git/ignore` and
`.git/info/exclude` are machine-local, and honoring them would render the same
repo differently on different machines. `exclude` is the separate, tracked-tree
case — a list of docs-dir-relative path prefixes (e.g. `["superpowers/"]`) for a
directory you keep in git but don't publish. Both apply. The rendered site lands
in `out_dir`.

By default `build` is strict: an unresolvable internal link — or an image whose
file isn't there — fails the build. Pass `--lenient` to publish anyway, rendering
either as an honest 404, for unattended pipelines that must never miss an update
over one bad link. `serve` is always lenient, for the same reason.

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
