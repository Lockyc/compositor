# Follow-ups / known limitations

Deferred work and accepted limitations, with enough context to pick up later.
Not bugs that block use — conscious deferrals.

## `serve`

- **`compositor.toml` changes are not watched.** The `serve` watcher watches
  only the docs dir and captures the loaded config by value, so edits to
  `compositor.toml` (e.g. `site_name`) take effect only on restart. Fine for
  content editing; revisit if config becomes something you tune live.

- **A panic while handling a request would abort the process.** The request
  loop runs on the main thread and uses `.expect()` on the state lock. No
  reachable panic path from external input exists today (the lock's critical
  sections are panic-free, so it can't poison; the only other `expect` is a
  static, always-valid header), so this is latent, not live. Defense-in-depth
  for the unattended charter: isolate each request (`catch_unwind`) or move the
  serve loop off the main thread so one bad request can't take the server down.

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
- **No embeds / transclusion.** `![[Name]]` is not supported (renders as an image-style
  wikilink comrak won't resolve). Deferred; add if an the docs KB needs inline embeds.
- **Anchors are not validated.** `[[Name#section]]` appends `#section` to the href
  without checking the target page actually has that heading id — consistent with the
  existing `.md#frag` passthrough. A wrong anchor lands on the page top, not an error.
