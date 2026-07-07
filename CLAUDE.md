# compositor

## Purpose

A Rust static-site generator for Markdown doc repos, replacing MkDocs across the
the docs sites. `compositor build <dir>` renders a directory of Markdown into a themed,
tree-navigated, Pagefind-indexed static HTML site.

**Designed to run unattended.** The sites compositor serves rebuild without a human
watching a terminal, so the tool must **degrade gracefully on content errors, never
halt or swallow updates**. This splits the two commands' failure policy:

- **`build`** — the one-shot path a human or CI watches — stays **strict**: an
  unresolvable internal link is a hard error that fails the build loudly.
- **`serve`** — the long-running unattended path — is
  **lenient**: it never halts on a content error. An unresolvable internal link
  still gets its `.md`→`.html` rewrite (surfacing as an honest 404), the rebuild
  always succeeds, and the freshest render always swaps in. A single broken link
  must never freeze the site at a last-good revision and silently swallow every
  later edit — the worst failure mode for a process no one is monitoring.

## Current state

Milestone 1 (plain-GFM `build`) is **complete**: `compositor build <dir>` renders a
Markdown tree end to end — correct titles, case-insensitive sorted tree nav,
`.md`→`.html` link rewrite, syntect highlighting, attribute-safe escaping, verbatim
copy of non-Markdown assets, and an optional Pagefind index.

Milestone 4 (the `serve` dev server) is also **complete**: `compositor serve`
watches the docs tree, rebuilds in memory on change (via the lenient link policy
described in Purpose above), serves the result over `tiny_http`, and live-reloads
every viewer's browser by polling a `/__reload` epoch endpoint.

Not yet built (later milestones): `!!!` admonitions + explicit-`nav` override (M2);
`[[wikilinks]]` + frontmatter-driven KB titles (M3); host rollout, retiring
`mkdocs-base` (M5). The Pagefind **search UI** (an input box
wired to `pagefind-ui`) is deferred to the theme-polish pass: the search index is
built now, but the rendered pages carry no search box yet. Known divergence from
MkDocs: filenames with spaces produce spaces in URLs (functional; slugification is a
deferred decision).

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
- Heading anchors (via comrak `header_ids`).
- Tree-derived nav (directories become sections, alphabetical, `index.md` first).
- Title resolution: `frontmatter.title` -> first `# H1` -> humanized filename.
- Non-Markdown files in the docs dir copied verbatim into the output, mirroring
  their relative path (images, downloads, data files a page links to), so those
  references resolve in the built site — as MkDocs does.

Explicitly **not** in Milestone 1 (later plans): `[[wikilinks]]`, `!!!`
admonitions, explicit-`nav` config override, **per-page TOC** and the **Pagefind
search UI** (both deferred to the theme-polish pass — heading anchors and the search
*index* ship in M1, the rendered TOC and search box do not), the `serve` dev server,
host rollout. No functionality duplicated from `docgate`: `build` fails only on an
unresolvable internal link (a render error) — orphan/graph auditing stays
docgate's.

## Branching & releases

Two long-lived branches: **`dev`** (the integration trunk — all work lands here) and
**`main`** (the release branch and public face, kept a clean ancestor of `dev` at
rest). Never commit code directly to `main`; it advances only by fast-forwarding to a
release commit. A documentation-only change may land on `main` directly and is then
forward-merged into `dev` so `dev ⊇ main` holds.

The version source of truth is the workspace `version` in the root `Cargo.toml`; the
binary self-reports it (`compositor --version`, via clap) and the `v<version>` git tag
matches it exactly — never restate the literal elsewhere. A shipped bump is a **GitHub
release** (bump the version, tag `v<version>` on the release commit, publish notes
summarising what shipped), never a bare tag. Still `0.x` while the CLI/config surface
moves.

## Commands

- Build: `cargo build`
- Test: `cargo test`
- Serve (live-reload): `cargo run -p compositor -- serve --dir <project>` (`--host`, `--port 8000`, `--open`)
- Pre-merge gate: `just gate` (fmt-check + clippy + tests)
