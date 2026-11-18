// editor.js reconstruction over real comrak output: each corpus page from
// `fixtures/editor-corpus.json` (written by the `editor_fixture` Rust test) is
// loaded into a linkedom DOM with edit mode on, and editor.js runs against it
// as it does in a browser.
"use strict";

const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { parseHTML } = require("linkedom");

const EDITOR_JS = fs.readFileSync(path.join(__dirname, "..", "assets", "editor.js"), "utf8");
const CORPUS = JSON.parse(
  fs.readFileSync(path.join(__dirname, "fixtures", "editor-corpus.json"), "utf8"),
);

// A page with editor.js loaded and edit mode on. Returns the window, its
// `#doc` blocks that carry an editor (rich or raw), and the original source.
function load(entry) {
  const payload = JSON.stringify(entry.payload).replace(/</g, "\\u003c");
  const { window, document } = parseHTML(
    `<!doctype html><html><head></head><body><header class="topbar">` +
      `<button type="button" class="edit-toggle">Edit</button></header>` +
      `<main>${entry.doc}</main>` +
      `<script type="application/json" id="__editsrc">${payload}</script></body></html>`,
  );
  const storage = { "compositor-edit": "1" };
  const context = {
    window,
    document,
    localStorage: {
      getItem: (k) => (k in storage ? storage[k] : null),
      setItem: (k, v) => {
        storage[k] = String(v);
      },
    },
    setTimeout,
    clearTimeout,
    fetch: () => new Promise(() => {}),
    location: { reload() {} },
  };
  vm.runInNewContext(EDITOR_JS, context);
  const blocks = Array.from(document.getElementById("doc").children).filter(
    (el) => el.dataset.editKind === "rich" || el.dataset.editKind === "raw",
  );
  return { window, document, blocks, source: entry.payload.source };
}

function label(el) {
  return `<${el.tagName.toLowerCase()} data-src-range=${el.dataset.srcRange}>`;
}

for (const entry of CORPUS) {
  test(`${entry.name}: the corpus has the blocks it is meant to exercise`, () => {
    const { blocks } = load(entry);
    const tags = new Set(blocks.map((el) => el.tagName));
    for (const t of ["H1", "H2", "P", "UL", "OL", "TABLE", "BLOCKQUOTE", "PRE"]) {
      assert.ok(tags.has(t), `no editable ${t}`);
    }
  });

  test(`${entry.name}: an unedited block marked dirty reconstructs byte-identically`, () => {
    const { window, blocks, source } = load(entry);
    const failures = [];
    for (const el of blocks) {
      el.dataset.dirty = "";
      const out = window.__compositorEditor.reconstruct();
      delete el.dataset.dirty;
      if (out !== source) {
        const before = source.split("\n");
        const after = out.split("\n");
        let i = 0;
        while (i < before.length && before[i] === after[i]) i++;
        failures.push(
          `${label(el)} line ${i + 1}: ${JSON.stringify(before[i])} -> ${JSON.stringify(after[i])}`,
        );
      }
    }
    assert.deepStrictEqual(failures, []);
  });

  test(`${entry.name}: editing one block's text changes only that block's lines`, () => {
    const { window, document, blocks, source } = load(entry);
    const para = blocks.find(
      (el) => el.tagName === "P" && el.textContent.startsWith("A paragraph after a list"),
    );
    assert.ok(para, "corpus paragraph after the list");
    para.lastChild.nodeValue += " Edited.";
    para.dataset.dirty = "";
    const out = window.__compositorEditor.reconstruct();
    const [a, b] = para.dataset.srcRange.split("-").map(Number);
    const before = source.split("\n");
    const after = out.split("\n");
    assert.deepStrictEqual(after.slice(0, a - 1), before.slice(0, a - 1), "lines above changed");
    assert.deepStrictEqual(after.slice(b), before.slice(b), "lines below changed");
    assert.ok(after.slice(a - 1, b).join("\n").includes("Edited."), "the edit landed");
    assert.ok(document.getElementById("doc"));
  });
}
