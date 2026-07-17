# compositor

## Purpose

A Rust static-site generator for Markdown doc repos, replacing MkDocs across a
set of documentation sites. `compositor build <dir>` renders a directory of Markdown into a themed,
tree-navigated static HTML site.

**There is no site search, and adding one has not been decided.** A Pagefind
integration was scaffolded in at M1 — inherited from MkDocs Material's defaults
rather than chosen — and put a search box in every page's chrome that depended on
a `pagefind` binary which was never installed on any build host. It therefore
never worked once: every site shipped a dead search box, and every build printed
`warning: pagefind not found on PATH`. It is now removed root and branch. Do not
reintroduce search (Pagefind or otherwise) as a missing piece or a tidy-up — a
capability that has never worked and was never chosen is a thing to delete, not
to finish. If search is ever wanted, that is a decision to be made first, not a
gap to be filled.

**compositor owns the entire shell; Markdown is purely content.** The page chrome,
navigation menu, link rewriting, titles, the home/landing page, and configuration
are all compositor's responsibility — handled with **sane defaults and graceful
degradation**, never pushed onto the author or required as config. A docs tree needs
no `compositor.toml` and no special files to work:

- **No config** → defaults are synthesized (`site_name` from the folder; docs from
  `docs/` if present, else the dir itself). A *malformed* `compositor.toml` is still a
  hard, named error — only a *missing* one falls back. (See `config::SiteConfig::load`.)
- **Gitignored paths** → skipped in rendering, asset-copy, and `serve`'s on-demand
  asset serving. Untracked scratch (`docs/superpowers/`, a `.claude/worktrees/`
  copy of the whole tree) is not site content, so no config is needed to keep it
  out — a docs repo with no `compositor.toml` at all still gets this. **Repo
  `.gitignore` files only**, never the global `~/.config/git/ignore` or
  `.git/info/exclude`: those are machine-local, and honoring them would render the
  same repo differently on a laptop than on a build host. A non-git directory
  ignores nothing (the graceful default a host app hits serving a bare Markdown
  folder). Gitignored means never rendered — there is deliberately no opt-out and
  no re-inclusion list; the way to publish a path is to stop ignoring it in git.
  (See `exclude::Excluder`.)
- **`exclude`** (optional, in `compositor.toml`) → a list of docs-dir-relative path
  prefixes skipped in both rendering and asset-copy, and honored by `serve`'s
  on-demand asset serving too — the `exclude_docs` analog, e.g.
  `exclude = ["superpowers/"]`. **Distinct from the gitignore rule above, and both
  apply:** gitignore hides *untracked* scratch, `exclude` hides a *tracked* tree
  kept in git but deliberately not published. Absent → nothing is excluded beyond
  what git already ignores.
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
`.md`→`.html` link rewrite, syntect highlighting, attribute-safe escaping, and
verbatim copy of non-Markdown assets.

Milestone 4 (the `serve` dev server) is also **complete**: `compositor serve`
watches the docs tree, rebuilds in memory on change (via the lenient link policy
described in Purpose above), serves the result over `tiny_http`, and live-reloads
every viewer's browser by polling a `/__reload` epoch endpoint. `serve`'s accepted
limitations and deferred hardening are in [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md)
— read it before extending `serve`.

The theme-polish pass has also landed: the shell is Pico.css-based, with a top bar
(brand, light/dark toggle that persists across reload), a left
tree-nav that marks the active page (`aria-current`), and a server-side per-page TOC
(h2/h3) with scroll-spy. Each page also carries a **prev/next pager** at the foot of
the content column (accent-outline buttons over the reading order — the flattened nav
with the landing page first; see `render_page::reading_order`) and a site **footer**
(a "Built with compositor" attribution).

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
                                 # No CLI/disk assumptions, so `serve` and host
                                 # apps reuse it.
  compositor/                   # CLI + library: config load, Pico-based theme wrap
                                 # (askama) served via a linked assets/compositor.css
                                 # + assets/compositor.js (no inline stylesheet),
                                 # and write out_dir. Its [lib] target
                                 # is the embedding surface — see below.
```

## The embedding surface (`serve_handle`)

compositor's `[lib]` target is a real public API, not an implementation detail of the binary.
`serve_handle(project_dir) -> ServeHandle` serves a site on an OS-assigned loopback port and returns
once bound; `ServeHandle::shutdown()` stops and joins the two threads it owns — the request loop
and the rebuild watcher — and `Drop` does the same. It is the non-blocking counterpart to
`run_serve`, and **both build from the same `setup()`** — that shared
path is load-bearing: two parallel serve loops would drift, and reimplementing serve in a host app is
the shadow that this API exists to prevent.

**A returned handle means bound, not healthy — degradation must be reported, not just survived.**
Graceful degradation (see Purpose) keeps a site serving when its watcher fails to start, and under
the CLI the reason goes to stderr where a human sees it. An embedded host has no stderr, so the same
degradation arrives as a site that serves forever and never reloads, with nothing to read it off.
`ServeHandle::live_reload()` is that channel, derived from the watcher the handle owns rather than
stored, so it cannot drift. The general rule for this API: whenever the graceful path swallows a
failure the CLI would have printed, the handle has to expose it — an embedded consumer only knows
what the type tells it.

lector (`github.com/lockyc/lector`) is the first consumer: one `ServeHandle` per doc-repo tab. So
compositor now has a **second build toolchain** — its own `rust-toolchain.toml` standalone, and
lector's pin when consumed as a git dep (rustup resolves from the dir `cargo` runs in, never from
`~/.cargo/git/checkouts/`). The two can diverge; lector's build is the drift detector, and the failure
is loud and local. Do not add a pin here to "fix" that — this repo's pin governs its own gate and CI.

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
- Server-side per-page TOC (h2/h3) with scroll-spy.

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
release** (curate the README, bump the version, tag `v<version>` on the release commit,
publish notes summarising what shipped), never a bare tag. Still `0.x` while the
CLI/config surface moves.

**Curating the `README.md` is part of cutting the release, not an extra.** The release
is when the public face actually gets read, so it is when the README is reconciled
against what compositor now *is*: status honest (nothing "coming soon" that already
shipped, nothing described that was removed), the feature list matching the built code,
the examples still runnable. A README describing the previous release is the most-read
stale doc here.

**A release publishes a Linux binary, and the consuming host's updater depends on it.**
Pushing the `v*` tag runs [`.github/workflows/release.yml`](.github/workflows/release.yml),
which builds `x86_64-unknown-linux-gnu` with the pinned toolchain, attaches it plus a
`.sha256`, and creates the release. The consuming host fetches that asset
**unauthenticated** (the reason this repo is public) and pushes it to the build hosts —
so a release whose asset is missing silently strands every consuming docs site on the
old binary. Tag, then **verify the release exists with both assets**; that check is the
release, not a formality.

**Why `x86_64-unknown-linux-gnu` only — the asset is a deploy artifact, not a
courtesy download.** It exists because the build hosts are small Debian containers
with *no Rust toolchain*, and putting one there — rustup plus a full workspace compile,
per release, on a 2-core/2 GB box — is strictly worse than shipping one self-contained
binary they can't build but can run. That is the whole reason this repo is public: an
unauthenticated fetch needs no credential on the host.

**There is deliberately no macOS (or any other) binary: nothing consumes one.** The two
real consumers are that Linux updater, and **lector, which depends on compositor as a
pinned git *crate* (`compositor = { git = …, rev = … }`) and builds it from source** —
it never fetches a release asset. On a Mac you have the repo, so `cargo build` is the
path; there is no macOS install story to serve. **Don't add targets to
`release.yml` for symmetry** — a matrix that publishes artifacts nobody fetches is
build time and maintenance bought for nothing. Add one when a consumer exists, and name
that consumer.

**Footgun — Actions can go silently deaf to `push` events.** After this repo was
deleted and re-created, `push` and tag events created **no workflow run at all**
(`actions/runs` → `total_count: 0`) while every visible setting looked healthy:
Actions enabled, both workflows `active`, `allowed_actions: all`, public, not a fork.
Only `workflow_dispatch` ran. The result is the dangerous kind of quiet — tagging
appears to succeed and simply publishes nothing, which reads as "the release is done"
until a consuming host is found still on the old binary. **The fix is to toggle
Actions off and back on** (`gh api -X PUT repos/<o>/<r>/actions/permissions -F
enabled=false`, then `=true`), which clears the stale state; push events fire
immediately after. Don't go hunting through workflow YAML for this — the workflows are
not the problem.

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

## CI triggers

**`just gate` is the real gate; CI is not a per-commit check.** CI runs on push to
`main`, on `pull_request`, and on `workflow_dispatch` — see `.github/workflows/ci.yml`.

**Footgun: don't add `dev` to CI's push triggers.** It reads as an obvious omission —
`dev` is the integration trunk, so surely it should be gated — which is exactly why this
note exists; the reasoning is wrong here and the mistake has been made. `dev` takes
frequent, deliberately half-finished pushes (the house rule is commit-as-you-go, WIP
commits and all), so gating each one burns CI minutes to report failures that are
expected and already known. Correctness on `dev` is held by `just gate` locally, run
before work is called done; CI's job is the release path (`main`) plus outside
contributions (`pull_request`). Use `workflow_dispatch` to run CI on `dev` on demand.
