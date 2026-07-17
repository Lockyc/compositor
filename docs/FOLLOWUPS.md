# Follow-ups / known limitations

Deferred work and accepted limitations, with enough context to pick up later.
Not bugs that block use — conscious deferrals.

## `serve`

- **`compositor.toml` changes are not watched.** The `serve` watcher watches
  only the docs dir and captures the loaded config by value, so edits to
  `compositor.toml` (e.g. `site_name`) take effect only on restart. Fine for
  content editing; revisit if config becomes something you tune live.

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

- **Search is unavailable under `serve`.** `serve` renders in memory and does not
  run Pagefind, so `/pagefind/*` 404s and the top-bar search box stays empty
  (a console 404 for `pagefind-ui.js`, no JS error). Search works in `build`
  output, which is what gets hosted. Wiring an in-memory index into `serve` is
  deferred.

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
