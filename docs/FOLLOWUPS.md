# Follow-ups / known limitations

Deferred work and accepted limitations, with enough context to pick up later.
Not bugs that block use — conscious deferrals.

## `serve`

- **`compositor.toml` changes are not watched.** The `serve` watcher watches
  only the docs dir and captures the loaded config by value, so edits to
  `compositor.toml` (e.g. `site_name`) take effect only on restart. Fine for
  content editing; revisit if config becomes something you tune live.

- **`.gitignore` changes are not watched.** The watcher watches only the docs dir,
  but the repo-root `.gitignore` sits outside it whenever `docs_dir` is a subdir.
  So editing `.gitignore` does not itself trigger a rebuild; the new rules land on
  the next rebuild from any docs edit, or on restart. The `Excluder` *is* rebuilt
  every cycle (see `rebuild_into`), so this is staleness of the *trigger*, not of
  the rules. Revisit by watching the collected `.gitignore` paths alongside the
  docs dir.

- **A repo-root image added after startup 404s until an unrelated docs edit
  triggers a rebuild.** The watcher watches only the docs dir, but a repo-root
  README/CLAUDE page's image can sit outside it (see `root_assets::RootAssets`).
  `serve`'s lenient policy renders a not-yet-existing one as an honest 404 at
  the moment it's referenced, and nothing outside the docs dir triggers the
  rebuild that would pick it up once the file lands — same family as the
  `compositor.toml` and `.gitignore` entries above. Correct-by-design (honest
  404, not a stale freeze; the next docs-triggered rebuild resolves it), not a
  bug. Revisit alongside those two if repo-root assets ever need their own watch.

- **A panic while handling a request kills that server, silently.** No reachable panic path from
  external input exists today (the lock's critical sections are panic-free, so it can't poison; the
  only other `expect`s are two static, always-valid headers), so this is latent, not live. What
  changed with `serve_handle` is the *blast radius*, and it now differs per entry point:
  - **`run_serve` (CLI):** the loop is the main thread — a panic aborts the process. Loud.
  - **`serve_handle` (embedded):** the loop is one background thread among N. A panic takes down that
    one site's server while the host still believes it is live. **Silent**, and the host cannot see it
    without checking.
  A host embedding this must detect a dead serve thread rather than trust the handle
  (lector drops its `live` dot on one). Defense-in-depth here: isolate each request (`catch_unwind`).
  The entry's older suggestion — "move the serve loop off the main thread" — is done for the embedded
  path and is not the fix for the CLI one.

- **The 404 page carries no live-reload script.** A tab showing a dead link's
  404 will not auto-reload when the missing file is later created (the *linking*
  page does reload, since it is a real injected page). Minor UX asymmetry,
  consistent with the honest-404 design. Could inject the reload script
  (baselined at the current epoch) into the 404 body if wanted.

- **Single-threaded request loop can head-of-line block.** `tiny_http`'s
  `incoming_requests()` is a single consumer and `respond()` writes
  synchronously, so a slow-reading client briefly blocks other tabs' reload
  polls. Acceptable ceiling for a local/loopback server; revisit only if `serve`
  is put in front of many concurrent real readers.
  Under `serve_handle` this is a non-issue by construction *for cross-site blocking*: one server per
  site, not one shared, so a slow reader on one site cannot block another's reload poll. The same
  synchronous write does give `serve_handle` a new consequence, though: unbounded shutdown latency.
  `ServeHandle::stop()` joins the rebuild thread first (which may be mid-rebuild on a large tree),
  then `unblock()`s and joins `serve_thread` — but `unblock()` only releases a thread parked in
  `recv()`; it cannot interrupt an in-flight `respond()`, which is synchronous. A client that
  requests a large asset and stops reading holds `shutdown()`/`Drop` until that write unsticks.
  Neither wait is bounded, though both finish given a cooperative client and a finite rebuild; low
  probability on loopback with small payloads, but matters most to a host calling `Drop` on a UI
  thread.

- **`serve`'s on-demand asset branch does not percent-decode the request path.**
  `GET /my%20image.png` 404s even for a plain docs asset that exists on disk as
  `my image.png` — the request url is used as a lookup/path key verbatim, the
  same shape of bug image resolution had (see `resolve_image` in
  `render-core/src/markdown.rs`, which now decodes before resolving). Pre-existing,
  independent of image resolution: it reproduces identically for an asset that
  predates it. `build`'s output is unaffected — a static host decodes the request
  normally; only `serve`'s own handler skips the step. Fix by percent-decoding the
  request path before it's used as a lookup key, mirroring `resolve_image`.
  **Constraint on that fix:** `is_safe` (`serve.rs:106`) validates the raw
  request url, and the on-disk branch then joins that same raw url onto the
  docs dir (`serve.rs:195`). Decoding the path *before* the safety check would
  let `%2e%2e%2f` become a literal `..` that `is_safe` had already waved
  through as safe — a directory-traversal hole. Any decode-then-lookup change
  must re-run `is_safe` (or an equivalent check) against the *decoded* path,
  not just the raw one.

- **`rewrite_link` (`render-core/src/markdown.rs:156`) never percent-decodes
  and only ever splits `#`, never `?`.** A link to a real file with a space —
  `[link](my%20page.md)` pointing at `my page.md` — hard-fails a strict build
  as an unresolvable link, even though the file exists. Pre-existing (dates to
  the original M1 link work), independent of image resolution, and out of
  scope here. It's the same bug class this branch just fixed for images one
  function away: `resolve_image` in the same module already splits
  `#`/`?` off an image url and percent-decodes the remaining path before
  resolving, and is the model to follow if this is ever fixed. Fixing it would
  converge the two split sites, at which point a shared `(path, suffix)`
  helper would earn its keep — today they genuinely diverge (`rewrite_link`
  also does `.md`->`.html`), so they're deliberately not collapsed.

## Repo-root image resolution (`RootAssets`)

- **`RootAssets`'s `Rewrite` can emit a raw `#`/`?` that came from the
  filename, not the author's markup.** `RootAssets` returns a decoded
  filesystem path, so `resolve_image`'s `format!("{u}{suffix}")` can splice a
  literal `#`/`?` from a *filename* into the emitted `src`. A repo-root README
  writing `![x](foo%23bar.png)` for a real file named `foo#bar.png` emits
  `src="foo#bar.png"`; the browser reads `#bar.png` as a fragment and 404s,
  even though the file is copied to `site/`. Comrak re-encodes a space in the
  common case, so this is narrow: only a literal `#` or `?` inside a filename
  triggers it. Only `Rewrite` is affected, and only `RootAssets` returns
  `Rewrite` — `DocsAssets` returns `Keep`, untouched. Without this resolution
  path such an image would not resolve at all, so it is a 404 either way for a
  pathological filename — an incomplete edge, not a regression. Fixing it needs
  percent-encoding against a correct path character set, which is easy to get
  subtly wrong, to serve a filename most sites will never have. Degrades
  visibly (a 404, not silent data loss) and the file is still copied.
  Revisit if a real site hits it.

- **`is_gitignored` only sees the `.gitignore` files `Excluder::new` collected.**
  `collect_gitignores` gathers the repo root, each directory between it and the
  docs dir, and each directory beneath the docs dir — deliberately not every
  directory in the repo (that walks `target/`, which Cargo seeds with a
  `.gitignore` containing `*`, and pruning would need the matchers still being
  built). So `is_gitignored` — which judges assets *outside* the docs tree for
  `RootAssets` — consults the repo-root `.gitignore` (the one that hides scratch
  in practice) but not a `.gitignore` sitting in an outside-docs directory, e.g.
  a repo-root `images/.gitignore`. An image only that file ignores would be
  copied into the site. Narrow: it needs a repo-root README to reference an
  image that a nested, outside-docs `.gitignore` hides. Fixing it means
  collecting matchers lazily per queried path, which buys a second resolution
  path for a case nobody has hit. Revisit if one shows up.

## Wikilinks (M3)

- **No shortest-unique-suffix path matching.** Path-qualified `[[dir/Name]]` matches
  the *exact* rel-path stem only. Obsidian's shortest-unique-suffix resolution
  (`[[Name]]` matching `a/b/Name` when unique) is not implemented; use the full path
  from the docs root, or a title/alias.
- **No embeds / transclusion.** `![[Name]]` is not supported — comrak's wikilink
  extension does not fire on the `!`-prefixed form (the `!` is consumed by the
  image-opening logic first), so it passes through as inert literal text
  (`![[Name]]`), never resolving as a link or image. Deferred; add if a consuming KB
  needs inline embeds.
- **Anchors are not validated.** `[[Name#section]]` appends `#section` to the href
  without checking the target page actually has that heading id — consistent with the
  existing `.md#frag` passthrough. A wrong anchor lands on the page top, not an error.

## `exclude`

- **A link from a kept page into an excluded subtree is an unresolvable link.**
  An excluded page is dropped from the known-URL set entirely, so a link to it
  from a page that *is* rendered has nothing to resolve against — under a
  **strict** `build` that's a hard error, same as any other broken link
  (`--lenient` renders it as a 404 instead). By design, not a bug: during a
  migration, either pass `--lenient` or remove the link.

## Admonitions

- **A TOC link to a heading inside a collapsed `???` doesn't auto-expand it.**
  Headings inside an admonition are included in the right-rail TOC (matching
  MkDocs' inclusion behavior). But when the heading lives inside a *collapsed*
  `???`/`???` admonition, its `<details>` renders closed, so clicking the TOC
  entry scrolls to an anchor inside hidden content and nothing visibly happens
  until the reader expands the block. MkDocs Material adds JS that opens the
  `<details>` on anchor navigation; compositor does not yet. Low-likelihood (h2/h3
  inside a callout is rare) UX papercut; add the open-on-anchor script to
  `compositor.js` if it comes up.

- **Accepted parsing edges (all low-likelihood, author-trusted input).** The
  preprocessor (`render-core/src/admonitions.rs`) is deliberately simple; these
  diverge from MkDocs only on unusual input and are left as-is:
  - Body capture is indentation-based and *fence-unaware*: a fenced code block in
    an admonition whose *closing* fence is de-indented below 4 spaces breaks
    capture (the stray ` ``` ` is then read as a new top-level fence).
  - An opener is recognised on the line *after* non-blank text with no blank line
    between — more permissive than python-markdown, which requires a preceding
    blank. Authors blank-line before `!!!`, so it rarely bites.
  - An `# h1` placed *inside* an admonition body can become the page-title
    candidate (`find_first_h1` walks the whole tree). h1-in-callout is unusual.
  - `split_title` garbles a title only when a `"` sits in the *class* portion of
    the opener (bizarre input); `deindent4` treats one leading tab as 4 spaces.
- **The `color-mix()` background tint has no fallback** for Safari <16.2 / older
  engines — the border and title still render (both plain `var()`), only the
  subtle tint is lost. Graceful, not a break.
