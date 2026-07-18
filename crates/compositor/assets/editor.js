// Inline-editing client: the global edit toggle, its persistence, and
// per-block classification of #doc's direct children into editable vs
// read-only. The serializer, DOM->Markdown reconstruction, and autosave land
// in a later task — this file only decides *what* is editable and wires the
// on/off switch; it never writes anything back to the server.
//
// Only injected on a page served with inline editing on (see
// `ServedSite::edit_enabled` in serve.rs), but this stays defensive: if the
// `#__editsrc` payload isn't present, do nothing.
(function () {
  "use strict";

  var payloadEl = document.getElementById("__editsrc");
  if (!payloadEl) return;

  var payload;
  try {
    payload = JSON.parse(payloadEl.textContent);
  } catch (e) {
    return; // malformed payload -- degrade to a no-op, never throw
  }

  var lineMap = payload.lineMap || [];
  var fmLines = payload.fmLines || 0;

  // Tags whose rendered block maps cleanly onto an editable region of
  // Markdown source. Matches the contract in task-8-brief.md / CLAUDE.md.
  var EDITABLE_TAGS = {
    P: true,
    H1: true,
    H2: true,
    H3: true,
    H4: true,
    H5: true,
    H6: true,
    UL: true,
    OL: true,
    TABLE: true,
    BLOCKQUOTE: true,
  };

  // --- Pure helpers -------------------------------------------------------
  // Kept small and dependency-free so a later Node smoke test can import
  // them directly if a JS harness is ever added (see CLAUDE.md/FOLLOWUPS).

  // Reads `data-sourcepos` ("startLine:startCol-endLine:endCol", 1-based,
  // preprocessed-body coordinates) off a rendered block. Returns null if the
  // attribute is absent or doesn't match comrak's format (e.g. an
  // admonition's raw wrapper `<div>`, which comrak can't annotate).
  function parseSourcepos(el) {
    var raw = el.getAttribute("data-sourcepos");
    if (!raw) return null;
    var m = /^(\d+):\d+-(\d+):\d+$/.exec(raw);
    if (!m) return null;
    return { startLine: parseInt(m[1], 10), endLine: parseInt(m[2], 10) };
  }

  // Maps a {startLine,endLine} (1-based, preprocessed-body coordinates) to
  // {a,b} 1-based FILE line numbers, or null if any body line the block
  // spans is unmapped (lineMap[i] === null -- a synthesized admonition
  // line with no original-source counterpart).
  //
  // Deliberately looks up *every* line in the range rather than mapping only
  // startLine and assuming a constant offset to endLine: an admonition
  // region shifts the output/source line count out of lockstep partway
  // through a block, so the offset at the top of a range is not guaranteed
  // to hold at the bottom.
  function toFileLines(range, lineMap, fmLines) {
    if (!range) return null;
    var a = null;
    var b = null;
    for (var line = range.startLine; line <= range.endLine; line++) {
      var idx = line - 1; // lineMap is 0-based per preprocessed-output line
      if (idx < 0 || idx >= lineMap.length) return null;
      var mapped = lineMap[idx];
      if (mapped === null || mapped === undefined) return null;
      var fileLine = mapped + fmLines + 1; // 1-based file line
      if (a === null) a = fileLine;
      b = fileLine;
    }
    return a === null ? null : { a: a, b: b };
  }

  // --- Classification -------------------------------------------------

  // Decides one #doc direct child's edit kind and stashes what the next
  // task needs to act on it. Idempotent -- safe to call more than once on
  // the same element.
  function classify(el) {
    delete el.dataset.noedit;
    delete el.dataset.editKind;
    delete el.dataset.srcRange;

    var range = parseSourcepos(el);
    var fileRange = range ? toFileLines(range, lineMap, fmLines) : null;
    var tag = el.tagName;

    if (range && fileRange && EDITABLE_TAGS[tag]) {
      el.dataset.editKind = "rich";
      el.dataset.srcRange = fileRange.a + "-" + fileRange.b;
      return;
    }

    if (tag === "PRE") {
      if (range && fileRange) {
        el.dataset.editKind = "raw";
        el.dataset.srcRange = fileRange.a + "-" + fileRange.b;
        return;
      }
      // A PRE without a mappable sourcepos (shouldn't normally happen, but
      // degrade gracefully rather than assume) is read-only.
      el.dataset.noedit = "";
      return;
    }

    // No sourcepos, an unmappable (admonition-involved) range, or a tag
    // outside the supported set -- read-only.
    el.dataset.noedit = "";
  }

  function classifyAll() {
    var doc = document.getElementById("doc");
    if (!doc) return;
    Array.prototype.forEach.call(doc.children, classify);
  }

  // Applies/removes `contenteditable` on "rich" blocks per the current edit
  // mode. Raw (PRE) blocks are left alone here -- the next task owns their
  // raw-source swap, which is not a plain `contenteditable` toggle.
  function applyEditableAttrs(on) {
    var doc = document.getElementById("doc");
    if (!doc) return;
    Array.prototype.forEach.call(doc.children, function (el) {
      if (el.dataset.editKind !== "rich") return;
      if (on) el.setAttribute("contenteditable", "true");
      else el.removeAttribute("contenteditable");
    });
  }

  // Classify on load unconditionally: the srcRange stash and the noedit
  // marker (used by editor.css) apply regardless of whether edit mode
  // happens to be on right now.
  classifyAll();

  // --- Global toggle + persistence ----------------------------------------
  // Same `localStorage` + guarded-try style as compositor.js's theme
  // anti-flash toggle: persistence must never throw in a context where
  // storage is unavailable (private browsing, disabled storage, etc).

  var STORAGE_KEY = "compositor-edit";
  var toggle = document.querySelector(".edit-toggle");
  var body = document.body;

  function isOn() {
    try {
      return localStorage.getItem(STORAGE_KEY) === "1";
    } catch (e) {
      return false;
    }
  }

  function setOn(on) {
    try {
      localStorage.setItem(STORAGE_KEY, on ? "1" : "0");
    } catch (e) {}
    body.classList.toggle("editing", on);
    if (toggle) toggle.setAttribute("aria-pressed", on ? "true" : "false");
    if (on) classifyAll(); // (re)apply classification whenever edit mode turns on
    applyEditableAttrs(on);
  }

  // Restore persisted state on load -- this matters because a save in a
  // later task can still cause a full reload, and edit mode must survive it.
  setOn(isOn());

  if (toggle) {
    toggle.addEventListener("click", function () {
      setOn(!isOn());
    });
  }
})();
