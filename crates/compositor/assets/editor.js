// Inline-editing client: the global edit toggle, its persistence, per-block
// classification of #doc's direct children into editable vs read-only, and the
// HTML->Markdown serializer + range-replacement reconstruction + autosave that
// write an edit back to disk.
//
// THE SACRED INVARIANT is byte-preservation: reconstruction is
// range-replacement on `payload.source` (the verbatim original file), never a
// walk-and-concat. Only the source lines belonging to blocks the user actually
// edited change; every other byte -- frontmatter, admonitions, code, the
// blank-line spacing between untouched blocks -- is byte-identical after a
// save. A `data-noedit` block's source lines are never touched.
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
  var source = typeof payload.source === "string" ? payload.source : "";
  var url = payload.url || "";

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
  // mode. Raw (PRE) blocks are handled by `applyRawEditable` -- their swap is a
  // content replacement (rendered highlight <-> literal source), not a plain
  // `contenteditable` toggle.
  function applyEditableAttrs(on) {
    var doc = document.getElementById("doc");
    if (!doc) return;
    Array.prototype.forEach.call(doc.children, function (el) {
      if (el.dataset.editKind !== "rich") return;
      if (on) el.setAttribute("contenteditable", "true");
      else el.removeAttribute("contenteditable");
    });
  }

  // Swaps every "raw" (PRE / code) block between its rendered, syntect-
  // highlighted HTML and the *literal* Markdown source lines it covers, so the
  // user edits the real source (fences and all) rather than highlight spans.
  // The literal text comes straight out of `payload.source` for the block's
  // `data-src-range`, which is exactly the span reconstruction will replace --
  // so whatever those file lines are (fence-inclusive or not per comrak's
  // sourcepos), edit and reconstruction stay self-consistent and byte-exact.
  //
  // `<pre>` is whitespace-preserving, so setting/reading `textContent` round-
  // trips newlines. The original rendered HTML is stashed so turning edit mode
  // off (without a save) restores the highlighted view; a save triggers a full
  // reload that re-renders it anyway (see the reconcile decision below).
  function applyRawEditable(on) {
    var doc = document.getElementById("doc");
    if (!doc) return;
    Array.prototype.forEach.call(doc.children, function (el) {
      if (el.dataset.editKind !== "raw") return;
      if (on) {
        if (el.__rawEditing) return;
        el.__rawEditing = true;
        el.__origHTML = el.innerHTML;
        el.textContent = rangeSourceText(el.dataset.srcRange);
        el.setAttribute("contenteditable", "true");
      } else {
        if (!el.__rawEditing) return;
        el.__rawEditing = false;
        el.removeAttribute("contenteditable");
        delete el.dataset.dirty;
        if (typeof el.__origHTML === "string") el.innerHTML = el.__origHTML;
      }
    });
  }

  // --- HTML -> Markdown serializer ---------------------------------------
  // Small, explicit, no external lib: covers exactly the supported constructs
  // (see CLAUDE.md's render surface). Anything outside the switch degrades to
  // its text content rather than throwing.

  // Serialize a block element's inline children to Markdown. Walks childNodes
  // rather than reading innerHTML so the inline mark set is an explicit switch.
  function serializeInline(el) {
    var out = "";
    var nodes = el.childNodes;
    for (var i = 0; i < nodes.length; i++) {
      var n = nodes[i];
      if (n.nodeType === 3) {
        // text node
        out += n.nodeValue;
      } else if (n.nodeType === 1) {
        var tag = n.tagName;
        if (tag === "STRONG" || tag === "B") {
          out += "**" + serializeInline(n) + "**";
        } else if (tag === "EM" || tag === "I") {
          out += "*" + serializeInline(n) + "*";
        } else if (tag === "CODE") {
          // Inline code is literal -- no nested mark parsing inside it.
          out += "`" + n.textContent + "`";
        } else if (tag === "A") {
          // getAttribute, not `.href`, to keep the author's relative URL
          // rather than the browser-resolved absolute one.
          var href = n.getAttribute("href") || "";
          out += "[" + serializeInline(n) + "](" + href + ")";
        } else if (tag === "IMG") {
          // An <img> has no children -- serialize its attributes directly, the
          // mirror of the A case. getAttribute keeps whatever src the DOM holds
          // (compositor rewrites image URLs at render, but an edit round-trip
          // must emit the current src, not a browser-resolved absolute one).
          var isrc = n.getAttribute("src") || "";
          var alt = n.getAttribute("alt") || "";
          out += "![" + alt + "](" + isrc + ")";
        } else if (tag === "DEL" || tag === "S") {
          out += "~~" + serializeInline(n) + "~~";
        } else if (tag === "BR") {
          out += "\n";
        } else {
          // Unknown inline element: transparent unwrap (recurse), which for a
          // plain wrapper equals its textContent but still surfaces any known
          // marks nested inside it.
          out += serializeInline(n);
        }
      }
    }
    return out;
  }

  // Serialize one #doc child block to its Markdown source. `null` means "no
  // representation" (an unknown/unsupported block) -- the caller drops it.
  function serializeBlock(el) {
    var tag = el.tagName;
    if (tag === "P") return serializeInline(el);
    if (/^H[1-6]$/.test(tag)) {
      var level = parseInt(tag.charAt(1), 10);
      return new Array(level + 1).join("#") + " " + serializeInline(el);
    }
    if (tag === "UL") return serializeList(el, false);
    if (tag === "OL") return serializeList(el, true);
    if (tag === "BLOCKQUOTE") return serializeBlockquote(el);
    if (tag === "TABLE") return serializeTable(el);
    if (tag === "PRE") {
      // Raw block: the editable region already holds the literal source lines
      // (see `applyRawEditable`), so its text IS the Markdown source.
      return el.textContent.replace(/\n$/, "");
    }
    return null;
  }

  function serializeList(el, ordered) {
    var out = [];
    var n = 0;
    var kids = el.children;
    for (var i = 0; i < kids.length; i++) {
      if (kids[i].tagName !== "LI") continue;
      n++;
      var marker = ordered ? n + ". " : "- ";
      out.push(marker + serializeListItem(kids[i]));
    }
    return out.join("\n");
  }

  // Serialize one <li>'s content. A GFM task item renders as a leading
  // `<input type="checkbox">` (comrak: `checked`/`disabled` present when
  // ticked) followed by the item text; detect it and emit the `[x]`/`[ ]`
  // token, dropping the input. serializeInline already drops the input itself
  // (unknown element, no children -> ""), so its content is just the text --
  // trim its leading space so `<input> done` round-trips to `[x] done`, not a
  // double space. A normal item has no leading checkbox and is unchanged.
  function serializeListItem(li) {
    var cb = leadingCheckbox(li);
    var body = serializeInline(li);
    if (cb) {
      return "[" + (cb.checked ? "x" : " ") + "] " + body.replace(/^\s+/, "");
    }
    return body;
  }

  // If the first meaningful child of `li` is a checkbox input, return
  // { checked }; otherwise null. Leading whitespace-only text is skipped; any
  // other leading node means it's not a task item.
  function leadingCheckbox(li) {
    var kids = li.childNodes;
    for (var i = 0; i < kids.length; i++) {
      var n = kids[i];
      if (n.nodeType === 3) {
        if (n.nodeValue.trim() === "") continue;
        return null;
      }
      if (n.nodeType === 1) {
        if (
          n.tagName === "INPUT" &&
          (n.getAttribute("type") || "").toLowerCase() === "checkbox"
        ) {
          return { checked: n.hasAttribute("checked") || n.checked === true };
        }
        return null;
      }
    }
    return null;
  }

  function serializeBlockquote(el) {
    // Serialize the inner block content, then prefix every line with "> "
    // (a blank line becomes ">"). Inner blocks are joined with a blank line.
    var parts = [];
    var kids = el.children;
    if (kids.length === 0) {
      parts.push(serializeInline(el));
    } else {
      for (var i = 0; i < kids.length; i++) {
        var s = serializeBlock(kids[i]);
        if (s === null) s = kids[i].textContent;
        parts.push(s);
      }
    }
    var body = parts.join("\n\n");
    return body
      .split("\n")
      .map(function (line) {
        return line.length ? "> " + line : ">";
      })
      .join("\n");
  }

  function serializeTable(el) {
    var rows = [];
    // Header row: prefer THEAD, fall back to the first row.
    var head = el.querySelector("thead tr");
    var headCells = head ? cellElementsOf(head) : [];
    if (headCells.length === 0) return el.textContent; // degrade, not throw
    rows.push(
      pipeRow(
        headCells.map(function (c) {
          return serializeInline(c);
        })
      )
    );
    // The GFM separator row carries per-column alignment, which comrak emits on
    // the header cells (verified: an `align="left|center|right"` attribute; a
    // `style="text-align:..."` is handled too for robustness). No alignment
    // keeps the plain `---`.
    rows.push(pipeRow(headCells.map(alignMarker)));
    var bodyRows = el.querySelectorAll("tbody tr");
    for (var i = 0; i < bodyRows.length; i++) {
      rows.push(
        pipeRow(
          cellElementsOf(bodyRows[i]).map(function (c) {
            return serializeInline(c);
          })
        )
      );
    }
    return rows.join("\n");
  }

  function cellElementsOf(tr) {
    var out = [];
    var kids = tr.children;
    for (var i = 0; i < kids.length; i++) {
      var t = kids[i].tagName;
      if (t === "TH" || t === "TD") out.push(kids[i]);
    }
    return out;
  }

  // The GFM alignment marker for a header cell: left `:--`, center `:-:`,
  // right `--:`, none `---`. Reads comrak's `align` attribute, falling back to
  // a `text-align` in an inline style.
  function alignMarker(cell) {
    var a = (cell.getAttribute("align") || "").toLowerCase();
    if (!a) {
      var m = /text-align:\s*(left|center|right)/i.exec(
        cell.getAttribute("style") || ""
      );
      if (m) a = m[1].toLowerCase();
    }
    if (a === "left") return ":--";
    if (a === "center") return ":-:";
    if (a === "right") return "--:";
    return "---";
  }

  function pipeRow(cells) {
    return "| " + cells.join(" | ") + " |";
  }

  // --- Line handling for range-replacement --------------------------------

  // Split into lines on "\n" ONLY, so a rejoin with "\n" is byte-exact for both
  // "\n" and "\r\n" files: a "\r" stays attached to its line's content and is
  // never touched. A trailing newline survives too ("a\nb\n" -> ["a","b",""]).
  // Only lines a dirty region *replaces* get fresh "\n"-delimited serialization;
  // every untouched line is reproduced verbatim.
  function splitLines(s) {
    return s.split("\n");
  }

  // A line is "blank" if it holds only whitespace (empty, spaces/tabs, or a lone
  // `\r` from a CRLF file). Used to peel the trailing blank-line run comrak folds
  // into a block's sourcepos so range-replacement preserves it verbatim.
  function isBlankLine(line) {
    return line.trim() === "";
  }

  // The literal source text for a "a-b" (1-based, inclusive FILE lines) range.
  function rangeSourceText(rangeStr) {
    var r = parseRange(rangeStr);
    if (!r) return "";
    return splitLines(source).slice(r.a - 1, r.b).join("\n");
  }

  function parseRange(rangeStr) {
    if (!rangeStr) return null;
    var m = /^(\d+)-(\d+)$/.exec(rangeStr);
    if (!m) return null;
    var a = parseInt(m[1], 10);
    var b = parseInt(m[2], 10);
    if (a < 1 || b < a) return null;
    return { a: a, b: b };
  }

  // --- Reconstruction (range-replacement on payload.source) ---------------

  // Classify one current #doc child for reconstruction:
  //  - "boundary": its source lines are preserved verbatim and it bounds a
  //    region (a data-noedit block, or a mapped rich/raw block NOT marked
  //    dirty).
  //  - "member": belongs to a dirty region (a dirty mapped block, or a NEW
  //    unmapped block e.g. a paragraph split off with Enter).
  function reconKind(el) {
    if (el.dataset.noedit !== undefined) return "boundary";
    var mapped = el.dataset.editKind === "rich" || el.dataset.editKind === "raw";
    if (mapped && el.dataset.dirty === undefined) return "boundary";
    return "member";
  }

  // Rebuild the full file source from the current DOM. Range-replacement, never
  // walk-and-concat: consecutive non-boundary #doc children are grouped into
  // dirty regions bounded by untouched/noedit blocks (and the document edges);
  // each region's source line span (min-start .. max-end of its mapped blocks'
  // ranges) is replaced by the serialized Markdown of that region's current
  // blocks. Regions are applied in DESCENDING start order so earlier line
  // indices stay valid. Nothing dirty -> `source` returned unchanged.
  function reconstruct() {
    var doc = document.getElementById("doc");
    if (!doc) return source;
    if (!anyDirty()) return source;

    var children = Array.prototype.slice.call(doc.children);
    var regions = [];
    var current = null;
    for (var i = 0; i < children.length; i++) {
      var el = children[i];
      if (reconKind(el) === "boundary") {
        current = null; // a boundary closes any open region
        continue;
      }
      if (!current) {
        current = [];
        regions.push(current);
      }
      current.push(el);
    }

    var lines = splitLines(source);
    var edits = [];
    for (var r = 0; r < regions.length; r++) {
      var region = regions[r];
      var span = regionSpan(region);
      if (!span) continue; // no mapped block in the region -> can't place it
      var text = serializeRegion(region);
      edits.push({ a: span.a, b: span.b, text: text });
    }

    // Descending start line so a splice never shifts a not-yet-applied span.
    edits.sort(function (x, y) {
      return y.a - x.a;
    });
    for (var e = 0; e < edits.length; e++) {
      var ed = edits[e];
      var count = ed.b - ed.a + 1; // 1-based inclusive [a,b] -> line count
      var original = lines.slice(ed.a - 1, ed.a - 1 + count);
      // Peel the trailing run of blank lines off the ORIGINAL span and keep it
      // verbatim. comrak's sourcepos for a list (and some other blocks) includes
      // the blank separator line that follows it; the serialized replacement has
      // no such trailer, so re-appending the peeled blanks preserves the byte-
      // exact spacing between this block and the untouched one after it. A block
      // whose span has no trailing blank (a one-line paragraph/heading) peels an
      // empty trailer and is unaffected. Whitespace-only counts as blank; a `\r`
      // (CRLF) or spaces ride along untouched, never normalized.
      var trailerStart = original.length;
      while (trailerStart > 0 && isBlankLine(original[trailerStart - 1])) {
        trailerStart--;
      }
      var trailer = original.slice(trailerStart);
      // A region that serializes to nothing (every block deleted/emptied)
      // removes its source lines entirely -- trailer and all -- rather than
      // stranding orphan blank lines: splice with no inserts.
      var replacement = ed.text.length
        ? splitLines(ed.text).concat(trailer)
        : [];
      lines.splice(ed.a - 1, count, ...replacement);
    }
    return lines.join("\n");
  }

  // A region's source span: min start .. max end across its blocks that carry a
  // data-src-range. New unmapped blocks contribute no bound (they are placed
  // within the mapped span by DOM order). `null` if no block is mapped.
  function regionSpan(region) {
    var a = null;
    var b = null;
    for (var i = 0; i < region.length; i++) {
      var rng = parseRange(region[i].dataset.srcRange);
      if (!rng) continue;
      if (a === null || rng.a < a) a = rng.a;
      if (b === null || rng.b > b) b = rng.b;
    }
    return a === null ? null : { a: a, b: b };
  }

  // Serialize a region's current blocks in DOM order, dropping any that have no
  // representation (unknown block) or serialize to empty (a deleted/cleared
  // block contributes nothing), joined by a blank line.
  function serializeRegion(region) {
    var parts = [];
    for (var i = 0; i < region.length; i++) {
      var s = serializeBlock(region[i]);
      if (s === null) continue;
      if (s.length === 0) continue;
      parts.push(s);
    }
    return parts.join("\n\n");
  }

  function anyDirty() {
    var doc = document.getElementById("doc");
    if (!doc) return false;
    return Array.prototype.some.call(doc.children, function (el) {
      return el.dataset.dirty !== undefined;
    });
  }

  // Whether any currently-dirty block is a raw (code) block -- decides the
  // reconcile: a raw edit must let the reload through so syntect re-highlights.
  function anyRawDirty() {
    var doc = document.getElementById("doc");
    if (!doc) return false;
    return Array.prototype.some.call(doc.children, function (el) {
      return el.dataset.dirty !== undefined && el.dataset.editKind === "raw";
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
    applyRawEditable(on);
  }

  // Restore persisted state on load -- this matters because a save can cause a
  // full reload (a raw/code edit, or an external change) and edit mode must
  // survive it.
  setOn(isOn());

  if (toggle) {
    toggle.addEventListener("click", function () {
      setOn(!isOn());
    });
  }

  // --- Autosave -----------------------------------------------------------
  // On input, mark the edited block dirty (divergence from `payload.source`,
  // which stays the ORIGINAL file for the life of this page) and schedule a
  // debounced save. On blur of an editable block, save immediately. Both go
  // through `save()`, which POSTs the reconstructed full file to `/__edit`.
  //
  // `data-dirty` is never cleared while the page lives -- it records divergence
  // from `payload.source`, so a second edit after a suppressed (prose) save
  // reconstructs ALL edits against the still-original source and writes a
  // complete, correct file. `pendingUnsaved` (a separate flag) gates whether a
  // save actually needs to fire, so a bare blur with nothing new doesn't re-POST.

  var SAVE_DEBOUNCE_MS = 800;
  var saveTimer = null;
  var pendingUnsaved = false;
  // Set true after a successful pure-prose save; the reload hook consumes it to
  // suppress exactly one self-inflicted epoch bump (the DOM already shows the
  // edit). A raw save leaves it false so the reload proceeds and re-highlights.
  var suppressReload = false;

  var doc = document.getElementById("doc");

  function markDirtyFromEvent(ev) {
    if (!body.classList.contains("editing")) return;
    var el = childBlockOf(ev.target);
    if (!el) return;
    var kind = el.dataset.editKind;
    // A block from an Enter-split is a new #doc child with no editKind; still
    // mark it dirty so its region is reconstructed. A data-noedit block never
    // becomes dirty.
    if (el.dataset.noedit !== undefined) return;
    if (kind === undefined && el.parentNode && el.parentNode.id !== "doc") return;
    el.dataset.dirty = "";
    pendingUnsaved = true;
  }

  // The #doc *direct child* an event's target sits inside (or is).
  function childBlockOf(node) {
    if (!doc) return null;
    var el = node;
    while (el && el.parentNode !== doc) {
      el = el.parentNode;
    }
    return el && el.parentNode === doc ? el : null;
  }

  function scheduleSave() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(function () {
      saveTimer = null;
      save();
    }, SAVE_DEBOUNCE_MS);
  }

  function save() {
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    if (!pendingUnsaved || !anyDirty()) return;
    var rawEdit = anyRawDirty();
    var newSource = reconstruct();
    pendingUnsaved = false;
    // Decide the reload disposition BEFORE the write lands: a pure-prose save
    // suppresses its own reload, a raw save forces it through (re-highlight).
    suppressReload = !rawEdit;
    fetch("/__edit", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url: url, source: newSource }),
    })
      .then(function (r) {
        if (!r.ok) {
          // The write did not land: don't claim a self-save, and let a future
          // reload through rather than swallowing it.
          pendingUnsaved = true;
          suppressReload = false;
        }
      })
      .catch(function () {
        pendingUnsaved = true;
        suppressReload = false;
      });
  }

  if (doc) {
    doc.addEventListener("input", function (ev) {
      markDirtyFromEvent(ev);
      if (pendingUnsaved) scheduleSave();
    });
    // focusout bubbles (blur does not) -- an editable block losing focus saves
    // immediately rather than waiting out the debounce.
    doc.addEventListener("focusout", function () {
      if (pendingUnsaved) save();
    });
  }

  // --- Self-save reconcile ------------------------------------------------
  // The reload script (see serve.rs `reload_script`) calls this on every epoch
  // change. Returning true suppresses the reload and adopts the new epoch as
  // baseline. We suppress exactly ONE bump per pure-prose save (the DOM already
  // shows it); a raw save or any EXTERNAL change returns false so the reload
  // proceeds. Bounded to a single suppression via the boolean, so an external
  // change is never permanently swallowed.
  window.__compositorBeforeReload = function (newEpoch) {
    if (suppressReload) {
      suppressReload = false;
      return true;
    }
    return false;
  };

  // Exposed for the end-to-end verification pass (next task) -- lets a driver
  // call reconstruct()/serializeBlock() directly. Not part of any runtime path.
  window.__compositorEditor = {
    reconstruct: reconstruct,
    serializeBlock: serializeBlock,
  };
})();
