// Node smoke harness for editor.js's pure serializer helpers: `node --test
// crates/compositor/tests/` (part of `just test` / `just gate`). Elements are
// minimal stand-ins exposing only the DOM surface the serializer reads.
"use strict";

const test = require("node:test");
const assert = require("node:assert");
const path = require("node:path");

const editor = require(path.join(__dirname, "..", "assets", "editor.js"));

function text(value) {
  return { nodeType: 3, nodeValue: value, childNodes: [] };
}

function el(tagName, attrs, children) {
  const kids = children || [];
  return {
    nodeType: 1,
    tagName,
    childNodes: kids,
    textContent: kids.map((k) => k.nodeValue || k.textContent || "").join(""),
    getAttribute: (name) => (name in (attrs || {}) ? attrs[name] : null),
  };
}

// A heading as comrak renders it: the empty permalink anchor, then the text.
function heading(level, id, children) {
  return el("H" + level, {}, [
    el("A", { href: "#" + id, "aria-hidden": "true", class: "anchor", id }, []),
    ...children,
  ]);
}

test("heading keeps its explicit {#id} when its text is edited", () => {
  const h = heading(3, "operating-set", [text("The operating set, revised")]);
  assert.strictEqual(
    editor.serializeHeading(h, "### The operating set {#operating-set}"),
    "### The operating set, revised {#operating-set}",
  );
});

test("the id block is carried over exactly as written", () => {
  const h = heading(2, "b", [text("B")]);
  assert.strictEqual(editor.serializeHeading(h, "## Old   { #b }"), "## B   { #b }");
  assert.strictEqual(editor.serializeHeading(h, "## Old {: #b }"), "## B {: #b }");
  assert.strictEqual(editor.serializeHeading(h, "## Old{#b}"), "## B{#b}");
  assert.strictEqual(editor.serializeHeading(h, "## Old {#b} ##"), "## B {#b}");
});

test("a setext heading's id is read from the text above its underline", () => {
  assert.strictEqual(editor.explicitIdSuffix("Title {#t}\n====="), " {#t}");
  assert.strictEqual(editor.explicitIdSuffix("Title\n-----"), "");
});

test("a heading inside a blockquote keeps its id", () => {
  assert.strictEqual(editor.explicitIdSuffix("> ## Quoted {#q}"), " {#q}");
});

test("a heading with no explicit id serializes without the anchor or a suffix", () => {
  const h = heading(2, "plain", [text("Plain "), el("EM", {}, [text("em")])]);
  assert.strictEqual(editor.serializeHeading(h, "## Plain *em*"), "## Plain *em*");
  assert.strictEqual(editor.serializeHeading(h, ""), "## Plain *em*");
});

test("a brace block render-core leaves literal is not treated as an id", () => {
  assert.strictEqual(editor.explicitIdSuffix("## Set {a, b}"), "");
  assert.strictEqual(editor.explicitIdSuffix("## Spaced {#two words}"), "");
  assert.strictEqual(editor.explicitIdSuffix("## Class {.wide}"), "");
});

test("the heading anchor is skipped but an ordinary link is kept", () => {
  const p = el("P", {}, [
    el("A", { href: "#x", "aria-hidden": "true", class: "anchor", id: "x" }, []),
    text("see "),
    el("A", { href: "other.md" }, [text("other")]),
  ]);
  assert.strictEqual(editor.serializeInline(p), "see [other](other.md)");
});
