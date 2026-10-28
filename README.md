# compositor

A Rust static-site generator for Markdown doc repos: point it at a directory of
Markdown and get a themed, navigable static site. A drop-in replacement for MkDocs,
with its render engine (`render-core`) also usable as a library.

[![Release](https://img.shields.io/github/v/release/Lockyc/compositor?sort=semver&label=release)](https://github.com/Lockyc/compositor/releases/latest)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-555)
[![Rust](https://img.shields.io/badge/rust-pinned%20(rust--toolchain.toml)-orange?logo=rust&logoColor=white)](rust-toolchain.toml)
[![License](https://img.shields.io/github/license/Lockyc/compositor)](LICENSE)

## Status

**In use, `0.x`.** The rendering is stable; the CLI and `compositor.toml` surface
may still change.

**Two commands.** `build` renders a docs tree to static HTML. `serve` is a
live-reload dev server: it watches the tree, rebuilds in memory on change, and
refreshes open browser tabs.

**Edit in place.** On a loopback `serve`, a topbar toggle flips the site into an
in-place editor: click a paragraph, heading, list, table, or code block and edit it
as rendered — changes autosave to the source `.md` and live-reload swaps the result
back in. If the file changes on disk while you're editing (another editor, an agent, a
checkout), the save is refused instead of overwriting it: your edit stays on screen with a
banner to copy it and reload. Only the blocks you touch are rewritten; admonitions, code, and frontmatter
around them stay byte-for-byte. It's off on any non-loopback bind
(`serve --host 0.0.0.0`) and never present in `build` output.

**What you get, with no config:** a Pico.css shell (top bar, light/dark toggle
that persists, a collapsible tree-nav that opens to and centres on the current
page, a per-page TOC with
scroll-spy, a prev/next pager, a footer); GFM with syntect-highlighted code;
MkDocs-style `!!!` admonitions and `???` collapsibles; MkDocs-style explicit heading
ids (`## Heading {#id}`); `[[wikilinks]]` resolved by title, filename, alias, or path
(`[[Page#id]]` deep-links to a heading); frontmatter `title`/`aliases`; `.md`→`.html`
link rewriting; and non-Markdown assets copied through verbatim. Image paths, in
`![](…)` or raw `<img src="…">`, resolve against the page that references them; in
the repo-root `README.md`, `CLAUDE.md` and `AGENTS.md` pages (below), against the repo root.
Raw HTML renders as-is, as in MkDocs: build only content you trust.

A repo-root `CLAUDE.md` and/or `AGENTS.md` (when the docs dir is a subdir) gets
its own top-level nav entry alongside Home (the repo-root `README.md`), both on by default. Set
`surface_claude_md = false` and/or `surface_agents_md = false` in
`compositor.toml` to hide either one. When both files are present and
`AGENTS.md`'s content is identical to `CLAUDE.md`'s (a symlink or a copy), only
the CLAUDE entry shows. Under the
loopback editor these surfaced repo-root pages — Home (`README.md`),
`CLAUDE.md`, and `AGENTS.md` — are themselves editable in place, the same as
any page under the docs dir.

**There is no site search.**

**As a library**, the `render-core` crate turns a Markdown tree into an in-memory
site model, and compositor's `[lib]` target exposes `serve_handle`/`ServeHandle`
— an embedding API that runs a site on a loopback port, for host apps supervising
many sites in one process, such as [lector](https://github.com/Lockyc/lector).

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
no runtime dependencies, for a Linux box with no Rust toolchain. Linux is the only prebuilt target —
everywhere else, build from source with the two lines above.

A `compositor.toml` is optional:

| Key | Default | What it does |
|---|---|---|
| `site_name` | *required* when the file exists | Site title in the top bar and `<title>` |
| `docs_dir` | `docs` | Docs tree to render, relative to `--dir` |
| `out_dir` | `site` | Where `build` writes the rendered site |
| `exclude` | `[]` | Docs-dir-relative path prefixes to skip (below) |
| `surface_claude_md` | `true` | Show a repo-root `CLAUDE.md` as a top-level nav entry |
| `surface_agents_md` | `true` | Show a repo-root `AGENTS.md` as a top-level nav entry |

Without one, defaults are synthesized: `site_name` from the folder name, and the
docs are taken from `docs/` if that subdir exists, else the directory itself — so
a bare folder of Markdown builds and serves with no config. In a `compositor.toml`,
an unknown key or a syntax/type error fails the run.

Paths your repo's `.gitignore` ignores are skipped — in rendering, asset-copy, and
`serve`'s on-demand asset serving and file watcher. Only the repo's own `.gitignore` files count (not
the global `~/.config/git/ignore` or `.git/info/exclude`), and outside a git repo
nothing is skipped. `exclude` covers files git does track: a
list of docs-dir-relative path prefixes (e.g. `["superpowers/"]`) for a directory
you keep in git but don't publish. Both apply. The rendered site lands in
`out_dir`, which must not be, or contain, the project or docs dir. An existing
`out_dir` must be empty or a previous compositor build — `build` refuses to replace
anything else — and a failed build leaves the previous site in place. Nothing under
`.git` is ever published or served.

By default `build` is strict: an unresolvable internal link — or an image whose
file isn't there — fails the build. Pass `--lenient` to publish anyway, rendering
either as a 404. Lenient also skips, with a warning, a page it can't read (not UTF-8,
or a dangling symlink); strict fails on it. `serve` is always lenient.

For local editing, `serve` watches the docs tree and live-reloads the browser on
every change:

```sh
./target/release/compositor serve --dir path/to/docs-repo --open
```

(`--dir` defaults to `.` and `--host` to `127.0.0.1`; omit `--port` to let the OS pick a free port —
the bound URL is printed on start — or pass `--port` to pin one. `--open` launches
the default browser. A loopback-bound `serve` answers only requests addressed to a
loopback host — `localhost`, `127.0.0.1`, `[::1]` — so to reach it through a proxy
under another name, bind a non-loopback `--host`.)

## Development

```sh
just build    # cargo build
just test     # cargo test + the editor.js Node harness
just gate     # fmt-check + clippy + cargo test + the Node harness (the pre-merge gate)
```

Branch model and release process live in [`CLAUDE.md`](CLAUDE.md).

## License

MIT — see [`LICENSE`](LICENSE).
