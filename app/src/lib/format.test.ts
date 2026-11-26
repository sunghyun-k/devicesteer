import assert from "node:assert/strict";
import { test } from "vitest";
import { clock, fileName, stamp } from "./format";

test("file names from either kind of path", () => {
  assert.equal(fileName("/Users/me/Movies/a b.mp4"), "a b.mp4");
  assert.equal(fileName("C:\\Users\\me\\a.png"), "a.png");
});

test("stamps sort by time", () => {
  assert.equal(stamp(new Date(2026, 10, 4, 8, 5, 9)), "2026-11-04 08.05.09");
});

test("the recording clock", () => {
  assert.equal(clock(7.9), "0:07");
  assert.equal(clock(754), "12:34");
  assert.equal(clock(3723), "1:02:03");
});
