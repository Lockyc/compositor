# compositor

## Purpose

A Rust static-site generator for Markdown doc repos, replacing MkDocs across a
set of documentation sites. `compositor build <dir>` renders a directory of Markdown into a themed,
tree-navigated, Pagefind-indexed static HTML site.

**compositor owns the entire shell; Markdown is purely content.** The page chrome,
navigation menu, link rewriting, titles, the home/landing page, and configuration
are all compositor's responsibility — handled with **sane defaults and graceful
degradation**, never pushed onto the author or required as config. A docs tree needs
no `compositor.toml` and no special files to work:

- **No config** → defaults are synthesized (`site_name` from the folder; docs from
  `docs/` if present, else the dir itself). A *malformed* `compositor.toml` is still a
  hard, named error — only a *missing* one falls back. (See `config::SiteConfig::load`.)
- **`exclude`** (optional, in `compositor.toml`) → a list of docs-dir-relative path
  prefixes skipped in both rendering and asset-copy, and honored by `serve`'s
  on-demand asset serving too — the `exclude_docs` analog, e.g.
  `exclude = ["superpowers/"]`. Absent → nothing is excluded (graceful default, same
  as no config at all).
- **No home page** → `/` always resolves to a working landing, first match wins:
  a docs-root `index`/`home`/`readme` (any case) is promoted; else the **repo-root
  `README.md`** is rendered (when the docs dir is a subdir, not the repo root itself);
  else a **generated index** — the site name over the nav as a link list. Never a blank
  body. (See `render_page::resolve_home`.)
- **Repo-root `CLAUDE.md`** → surfaced as a top-level nav entry (label `CLAUDE`,
  never content-derived), adjacent to Home. The nav-menu sibling of the README→home
  promotion: same repo-root discovery (`find_repo_root_md`) and lenient
  outside-the-docs-contract rendering, but a nav page rather than the landing. Only
  when the docs dir is a subdir (a docs-tree `CLAUDE.md` is already a normal page).
  (See `render_page::surface_repo_claude`.)
- **A broken internal link** → degrades to a 404 under `serve`, never a halt (below).

The Markdown author writes content; compositor supplies everything around it. When a
non-content concern has no obvious answer, pick the graceful default — don't error and
don't require the author to configure it.

**Designed to run unattended.** The sites compositor serves rebuild without a human
watching a terminal, so the tool must **degrade gracefully on content errors, never
halt or swallow updates**. This splits the two commands' failure policy:

- **`build`** — the one-shot path a human or CI watches — is **strict by
  default**: an unresolvable internal link is a hard error that fails the
  build loudly; `--lenient` opts out for unattended pipelines, rendering the
  broken link as an honest 404 instead.
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
every viewer's browser by polling a `/__reload` epoch endpoint. `serve`'s accepted
limitations and deferred hardening are in [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md)
— read it before extending `serve`.

The theme-polish pass has also landed: the shell is Pico.css-based, with a top bar
(brand, Pagefind search box, light/dark toggle that persists across reload), a left
tree-nav that marks the active page (`aria-current`), and a server-side per-page TOC
(h2/h3) with scroll-spy. Each page also carries a **prev/next pager** at the foot of
the content column (accent-outline buttons over the reading order — the flattened nav
with the landing page first; see `render_page::reading_order`) and a site **footer**
(a "Built with compositor" attribution). The search box is populated by the Pagefind
index built during `build`; it is unavailable under `serve` (which renders in memory
and never runs Pagefind) — see [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md).

Milestone 2 (admonitions) is also **complete**: MkDocs/Material `!!!` callouts and
`???`/`???+` collapsibles, with an arbitrary type word as the CSS class (known types
color-coded, unknown types gracefully default), an optional custom or empty title, and
nesting. A source preprocessor rewrites each block into an HTML wrapper whose body
still renders as Markdown in the single comrak pass — which requires comrak's raw-HTML
passthrough (`render.unsafe_ = true`), an intentional choice matching MkDocs: raw HTML
in author-trusted docs is allowed, not escaped.

Milestone 3 (`[[wikilinks]]` + frontmatter-driven KB titles) is **complete**:
`[[Name]]` resolves a page by name against a tree-wide index — frontmatter title,
filename stem (and its humanized form), `aliases`, or a path-qualified `[[dir/Name]]`
— with the page's resolved title (frontmatter `title` → first `# H1` → humanized
filename — the same chain as the "Title resolution:" bullet below) driving both
link identity and the rendered link text.
Matching is case-insensitive. `[[Name|label]]` overrides the text and `[[Name#anchor]]`
deep-links. Resolution honors the strict/lenient split: `build` hard-errors on an
unresolvable or ambiguous wikilink; `serve` picks the sorted-first candidate for an
ambiguous one and renders an unresolvable one as a visibly-dead `<a data-wikilink>`
that resolves on a later rebuild once the target exists.

Milestone 5 (host rollout + `mkdocs-base` retirement) is **complete**: compositor
is deployed to its build hosts and every consuming documentation site builds with
it; the `mkdocs-base` base-config repo has been deleted. The explicit-`nav` override
once planned for M2 was **dropped from the roadmap** — the auto-generated tree nav is
the only navigation. Known divergence from MkDocs: filenames
with spaces produce spaces in URLs (functional; slugification is a deferred decision).

## Layout

Cargo workspace, two crates:

```
Cargo.toml                      # [workspace] members
crates/
  render-core/                  # library: Markdown -> HTML, frontmatter, title
                                 # resolution, nav tree, link rewrite -> SiteModel.
                                 # No CLI/disk assumptions, so `serve` and a future
                                 # Tauri app can reuse it.
  compositor/                   # CLI crate: config load, Pico-based theme wrap
                                 # (askama) served via a linked assets/compositor.css
                                 # + assets/compositor.js (no inline stylesheet),
                                 # write out_dir, invoke Pagefind.
```

## Milestone-1 render surface (exhaustive)

- GFM: headings, lists, tables, task lists, autolinks, strikethrough, images,
  blockquotes.
- Fenced code with syntect highlighting.
- Frontmatter `title` and `aliases` keys (consume; ignore all other keys).
- Internal `.md` -> `.html` link rewrite.
- Heading anchors (via comrak `header_ids`).
- Tree-derived nav (directories become sections, alphabetical, `index.md` first).
- Title resolution: `frontmatter.title` -> first `# H1` -> humanized filename.
- Non-Markdown files in the docs dir copied verbatim into the output, mirroring
  their relative path (images, downloads, data files a page links to), so those
  references resolve in the built site — as MkDocs does.
- Server-side per-page TOC (h2/h3) with scroll-spy, and the Pagefind search UI
  wired into the top bar (search works in `build` output; unavailable under
  `serve`, see [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md)).

Explicitly **not** in Milestone 1: host rollout (M5).
(`[[wikilinks]]`, admonitions, the `serve` dev server, and the M5 host rollout have
all since landed; the explicit-`nav` override was dropped.)
No functionality duplicated from `docgraph`: `build` fails only on an unresolvable
internal link (a render error) — orphan/graph auditing stays docgraph's.

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
- Render a site: `cargo run -p compositor -- build --dir <project>` — strict by
  default (an unresolvable internal link fails the build); `--lenient` publishes
  anyway, rendering broken links as honest 404s, for unattended pipelines.
- Serve (live-reload): `cargo run -p compositor -- serve --dir <project>` (`--host`; `--port` omitted → OS picks a free port, printed on start; `--open`)
- Pre-merge gate: `just gate` (fmt-check + clippy + tests)

## Toolchain

`rust-toolchain.toml` is the single source of truth for the Rust version — `just
gate` and both GitHub workflows resolve from it (rustup reads it automatically),
so a green gate locally means a green CI. Bump it deliberately, fixing any new
clippy lints in the same change.

**Footgun: don't reintroduce a floating toolchain in CI.** A
`dtolnay/rust-toolchain@stable` step overrides the file and silently restores the
drift the pin exists to remove — a new stable lands a new clippy lint, CI goes red
with no code change, and the local gate still passes on the older compiler. That
cost a patch release once. The workflows install the pinned toolchain with `rustup
show` for exactly this reason.
