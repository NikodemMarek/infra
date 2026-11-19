import test from "node:test";
import assert from "node:assert/strict";
import { kindOf, toEvent } from "./event.js";

test("kindOf", () => {
  assert.equal(kindOf("hook@t.example.com"), "hook");
  assert.equal(kindOf("HOOK@t.example.com"), "hook");
  assert.equal(kindOf("github.com@t.example.com"), "throwaway");
  assert.equal(kindOf("hook.x@t.example.com"), "throwaway");
});

test("toEvent throwaway formats sender and collapses whitespace", () => {
  assert.deepEqual(
    toEvent("throwaway", "github.com@t.example.com", "b@x", {
      from: { name: "GitHub", address: "noreply@github.com" },
      subject: "Hi",
      text: "Hello\n\n  world",
    }),
    {
      kind: "throwaway",
      from: "GitHub <noreply@github.com>",
      to: "github.com@t.example.com",
      subject: "Hi",
      snippet: "Hello world",
    },
  );
});

test("toEvent hook uses parsed addresses and empty defaults", () => {
  assert.deepEqual(
    toEvent("hook", "hook@t.example.com", "s@gmail.com", {
      from: { address: "a@b.c" },
      to: [{ address: "me@gmail.com" }],
    }),
    { kind: "hook", from: "a@b.c", to: "me@gmail.com", subject: "", snippet: "" },
  );
});

test("toEvent hook falls back to envelope addresses", () => {
  const e = toEvent("hook", "hook@t.example.com", "s@gmail.com", {});
  assert.equal(e.to, "hook@t.example.com");
  assert.equal(e.from, "s@gmail.com");
});

test("toEvent snippet from html strips tags, style and entities", () => {
  const e = toEvent("hook", "hook@t.example.com", "s@x", {
    html: "<p>Code: <b>123</b> &amp; more</p><style>p{}</style>",
  });
  assert.equal(e.snippet, "Code: 123 & more");
});

test("toEvent snippet is capped at 500 chars", () => {
  const e = toEvent("hook", "hook@t.example.com", "s@x", { text: "a".repeat(600) });
  assert.equal(e.snippet.length, 500);
});
