---
type: architecture
---

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
- **Repo-root `CLAUDE.md` and `AGENTS.md`** → each surfaced as a top-level nav
  entry (label `CLAUDE`/`AGENTS`, never content-derived), adjacent to Home, nav
  order CLAUDE then AGENTS. The nav-menu sibling of the README→home promotion:
  same repo-root discovery (`find_repo_root_md`) and lenient outside-the-docs-contract
  rendering, but a nav page rather than the landing. Only when the docs dir is a
  subdir (a docs-tree file of the same name is already a normal page). Both surface
  by default; each is independently suppressible in `compositor.toml` via
  `surface_claude_md` / `surface_agents_md` (both default `true`). When both files
  are present and `AGENTS.md`'s raw content is identical to a surfaced `CLAUDE.md`'s
  (a symlink or a byte-identical copy), the AGENTS entry is dropped so the nav
  carries no duplicate — dedup only suppresses a duplicate, so toggling CLAUDE off
  still lets an identical AGENTS.md surface on its own. (See
  `render_page::surface_repo_agent_files`.)
- **Images a repo-root README.md/CLAUDE.md/AGENTS.md references** resolve against the **repo
  root** (what those urls are actually relative to): one landing inside the docs
  dir is rewritten to its docs url, one outside is copied into the site mirroring
  its repo-relative path — referenced files only, never a wholesale copy of the
  repo. Docs content wins a url collision, and the `Excluder` still applies. (See
  `root_assets::RootAssets`.)
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

An unresolvable **image** is treated the same as an unresolvable link: a hard
error under `build`, an honest 404 under `--lenient` and `serve`. This holds on
the repo-root README/CLAUDE/AGENTS pages too, whose *links* stay lenient (they sit
outside the docs link contract) but whose *images* either exist on disk under
the repo root or don't. (See `markdown::ImageResolver`.) Both a Markdown `![](…)`
**and** an author-written raw-HTML `<img src="…">` route through the resolver — see
the raw-HTML bullet under *Current state* for the coverage boundary.

## Current state

Everything compositor was built for is shipped: `build`, the `serve` dev server,
admonitions, `[[wikilinks]]`, and the host rollout — every consuming site builds with
it, and the `mkdocs-base` base-config repo it superseded is deleted. The README is the
tour of what that gets you; this section is the map from feature to the code that owns
it, plus the invariants that code doesn't state on its own.

**Start here for deferred work and accepted limitations:**
[`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md) is the register for the whole tree — it covers
`serve`, repo-root asset resolution (`RootAssets`), wikilinks, `exclude`, and
admonitions. Read the relevant section before extending any of them: each entry is a
conscious deferral recorded with its rationale, so an unread one gets rediscovered as a
bug and "fixed" against the reasoning that deferred it.

| Surface | Owns it |
| --- | --- |
| Markdown → `SiteModel`, link rewrite, image resolution | `render-core/src/markdown.rs`, `site.rs` |
| Admonition preprocessor | `render-core/src/admonitions.rs` |
| Wikilink index + resolution | `render-core/src/wikilink.rs` |
| Tree nav (tree model) | `render-core/src/nav.rs` |
| Nav collapse/expand rendering | `compositor/src/render_page.rs` (`nav_to_html`, `node_html`, `section_contains_url`) |
| `.gitignore` / `exclude` gating | `render-core/src/exclude.rs` |
| Theme wrap, per-page TOC, prev/next pager | `compositor/src/render_page.rs` (`reading_order`) |
| Repo-root asset resolution | `compositor/src/root_assets.rs` (`RootAssets`) |
| `serve`, live-reload, the embedding API | `compositor/src/serve.rs` (`setup`, `serve_handle`) |
| Serve-mode inline editing (loopback only) | `compositor/src/serve.rs` (`inject_editor`, `/__edit`, `edit_enabled`), `compositor/assets/editor.js` |

- **Raw HTML passes through (`render.unsafe_ = true`) — load-bearing, not incidental.**
  The admonition preprocessor rewrites each block into an HTML wrapper whose body must
  still render as Markdown in the *single* comrak pass; escaping raw HTML breaks that
  mechanism. It also matches MkDocs: raw HTML in author-trusted docs is allowed. Don't
  "harden" this without replacing the preprocessor first. **Because it passes through
  untouched, an author-written `<img src="…">` (READMEs use it to set `width=`) would
  otherwise bypass image resolution entirely — its relative `src` never rewritten, its
  asset never copied, so it 404s.** `render_inner` closes that by walking `HtmlBlock`/
  `HtmlInline` nodes and routing each **quoted `src`** through the same `resolve_image`
  as `![](…)` (`rewrite_html_assets` in `markdown.rs`), so it gets identical resolve-
  and-copy treatment. Deliberately narrow: `srcset` (and the `<source srcset>` dark/light
  `<picture>` idiom), unquoted values, and entity-escaped urls are **deferred** — the
  `<img src>` fallback still resolves, so a `<picture>` renders its default variant;
  add srcset parsing when a repo needs the responsive variant served.
- **Navigation is auto-generated from the file tree today.** An explicit-`nav` override was
  planned for M2 and **deferred** (not rejected) — revisit if a site needs manual section
  ordering; the auto-generated tree nav is the default until then.
- **Wikilink resolution honors the strict/lenient split** (see Purpose): `build`
  hard-errors on an unresolvable *or ambiguous* wikilink; `serve` picks the sorted-first
  candidate for an ambiguous one and renders an unresolvable one as a visibly-dead
  `<a data-wikilink>` that resolves on a later rebuild once the target exists.
- **Known divergence from MkDocs: filenames with spaces produce spaces in URLs.**
  Functional; slugification is a deferred decision, not an oversight.
- **Serve-mode inline editing is a loopback-only, serve-only capability.** On a
  loopback `serve`, each page carries a global **edit toggle** (topbar, persisted
  in `localStorage`) that turns the rendered docs-tree pages into in-place WYSIWYG
  editors; edits autosave to the source `.md` and flow back through live-reload. The
  invariants the code assumes:
  - **Loopback is the hard gate.** On a non-loopback bind (`serve --host 0.0.0.0`)
    the write endpoint `POST /__edit` is not registered and no toggle/payload/`editor.js`
    is injected — the same posture as the gitignore/exclude "don't hand out hidden files
    on `0.0.0.0`" rule. `build` output ships **none** of the edit scaffolding (no toggle,
    no `data-sourcepos`, no `#__editsrc` payload, no `editor.js`). (`edit_enabled` in
    `setup`, gated by `host_is_loopback`.)
  - **Block-scoped, byte-preserving reconstruction — never a whole-page HTML→Markdown
    pass** (which would corrupt admonitions, wikilinks, highlighted code, and
    frontmatter). The client rebuilds the file by **range-replacement on the verbatim
    original source** (shipped per page in the `#__editsrc` payload with a comrak
    source-position map); only the lines of blocks you actually edit change — everything
    else stays byte-identical. The map is trustworthy because `preprocess_admonitions_mapped`
    emits a passthrough line-map (`render-core`): comrak's positions see the *preprocessed*
    body, whose line count diverges from source across an admonition, so the map — not a
    frontmatter offset — is what converts a block's position to a real file line, and any
    admonition-touched block is marked `data-noedit`. (`assets/editor.js`.)
  - **v1 editable scope.** Docs-tree paragraphs/headings/lists/tables/blockquotes edit
    inline; fenced code edits as raw source; **admonitions and wikilink-dense blocks are
    read-only** (`data-noedit`) — see [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md) for why each
    is deferred. A surfaced page is editable whenever it has no *other* editable url to defer
    to: the repo-root README (served at `/`), CLAUDE, and AGENTS pages are inline-editable by
    this rule, while the generated index (no backing file) and the promoted docs-root
    `home`/`readme` `/`-alias — already editable at its own url — stay read-only.
  - **Writes are authorized by a server-built url→source map, never a client-named path.**
    `/__edit` accepts only a url already in `ServedSite.editable` and writes exactly the
    file that map recorded, atomically (temp-write + rename); a url with no backing source
    (a generated index, or the promoted docs-root `/`-alias) or off the map is refused. The
    map holds **only pages that carry an editor** (`edit_source.is_some()`) — which now
    includes the repo-root README/CLAUDE/AGENTS pages alongside the docs tree — so any other
    page's url is refused too; the map *is* the write boundary. `/__edit` also enforces
    **same-origin** (`Sec-Fetch-Site` `same-origin`/`none`, or `Origin` authority matching
    `Host`; a non-browser client sending neither is allowed): editing is on by default on
    loopback, so this closes the cross-origin-`fetch` vector a hostile page could otherwise
    use against the loopback port. The embedding surface is writable by default with a
    read-only opt-out — see below.

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

`serve_handle` is **writable by default** on its loopback bind — a host tab is edit-capable
(the toggle + `/__edit` are live) unless it opts out. `serve_handle_with(project_dir, editable)`
is that opt-out: `editable = false` forces `edit_enabled` off regardless of loopback, giving a
pure reader with no write surface. `serve_handle` delegates with `editable = true`. The
loopback-only write invariant (above) binds every consumer, embedded or CLI.

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

## Render surface (exhaustive)

- GFM: headings, lists, tables, task lists, autolinks, strikethrough, images,
  blockquotes.
- Fenced code with syntect highlighting.
- MkDocs/Material `!!!` callouts and `???`/`???+` collapsibles: an arbitrary type word
  becomes the CSS class (known types color-coded, unknown types gracefully default), an
  optional custom or empty title, and nesting.
- `[[wikilinks]]` resolved against a tree-wide index — frontmatter title, filename stem
  (and its humanized form), `aliases`, or a path-qualified `[[dir/Name]]`; the page's
  resolved title drives both link identity and rendered text. Matching is
  case-insensitive; `[[Name|label]]` overrides the text, `[[Name#anchor]]` deep-links.
- Frontmatter `title` and `aliases` keys (consume; ignore all other keys).
- Internal `.md` -> `.html` link rewrite.
- Heading anchors (via comrak `header_ids`).
- Tree-derived nav (directories become sections, alphabetical, `index.md` first),
  rendered as collapsible native `<details>`: the section(s) containing the current
  page render `open` server-side, per page (stateless — no JS, no persisted
  state); `generated_index`'s embedded nav renders with every section open via
  `nav_to_html`'s `expand_all` flag.
- Title resolution: `frontmatter.title` -> first `# H1` -> humanized filename.
- Non-Markdown files in the docs dir copied verbatim into the output, mirroring
  their relative path (images, downloads, data files a page links to), so those
  references resolve in the built site — as MkDocs does. Assets a repo-root
  README/CLAUDE/AGENTS page references from outside the docs dir are copied too, on
  reference (see Purpose).
- Server-side per-page TOC (h2/h3) with scroll-spy.

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
release** — never a bare tag. Still `0.x` while the CLI/config surface moves.

**Cut a release with `just release`** — the house recipe, the same shape docgraph and
mycelium use. Bump the workspace `version`, curate the README (below), write
`RELEASE_NOTES.md` (gitignored per-release scratch — an input to the recipe, never a
tracked file), commit, then run it. It refuses a missing notes file, a dirty tree, or an
existing tag *before* touching anything public; then runs the gate, pushes `dev`,
fast-forwards `main`, tags, creates the release with your notes, and waits for the Linux
binary to land.

**Never `gh release create` by hand, and never let CI create the release.** Two creators
race, and the loser publishes nothing. `release.yml` used to create it on tag push, so a
manual create won the race and left a release carrying **zero assets** — which reads
exactly like success. `just release` is the only creator; `release.yml` only *uploads*.

**Notes are written, not generated.** `--generate-notes` summarises merged PRs, and this
repo integrates by direct merge to `dev` and opens none — so it yields a bare compare link
that says nothing. mycelium shipped an empty release that way; don't repeat it.

**Curating the `README.md` is part of cutting the release, not an extra.** The release
is when the public face actually gets read, so it is when the README is reconciled
against what compositor now *is*: status honest (nothing "coming soon" that already
shipped, nothing described that was removed), the feature list matching the built code,
the examples still runnable. A README describing the previous release is the most-read
stale doc here.

**A release publishes a Linux binary, and the consuming host's updater depends on it.**
[`release.yml`](.github/workflows/release.yml) builds `x86_64-unknown-linux-gnu` with the
pinned toolchain and uploads it plus a `.sha256` onto the release `just release` already
created — triggered by `release: published`, never by the tag. It runs in CI for exactly
one reason: a Mac cannot build that target without cross tooling. The consuming host
fetches the asset **unauthenticated** (the reason this repo is public) and pushes it to
the build hosts, so a release whose asset never arrives silently strands every consuming
docs site on the old binary. `just release` blocks until both assets appear and fails
loudly if they don't — that wait **is** the release, not a formality.

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

**Footgun — Actions can go silently deaf to events.** After this repo was deleted and
re-created, `push` and tag events created **no workflow run at all** (`actions/runs` →
`total_count: 0`) while every visible setting looked healthy: Actions enabled, both
workflows `active`, `allowed_actions: all`, public, not a fork. Only `workflow_dispatch`
ran. **The fix is to toggle Actions off and back on** (`gh api -X PUT
repos/<o>/<r>/actions/permissions -F enabled=false`, then `=true`), which clears the stale
state. Don't go hunting through workflow YAML for this — the workflows are not the
problem. This no longer produces a *silent* empty release (`just release` waits on the
assets and fails), but the recovery still matters: re-attach with `gh workflow run
release.yml -f tag=v<version>`.

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
