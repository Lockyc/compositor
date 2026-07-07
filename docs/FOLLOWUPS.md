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
