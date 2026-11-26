// Unit tests for the computer-to-device key mapping (keymap.ts). Run with `npm test`.

import assert from "node:assert/strict";
import { test } from "vitest";
import { KeyMapper, type KeyStep } from "./keymap";

const d = (code: string): KeyStep => ({ code, down: true });
const u = (code: string): KeyStep => ({ code, down: false });

test("on a Mac every key is itself", () => {
  const m = new KeyMapper(false);
  assert.deepEqual(m.down("MetaLeft"), [d("MetaLeft")]);
  assert.deepEqual(m.down("ControlLeft"), [d("ControlLeft")]);
  assert.deepEqual(m.down("Space"), [d("Space")]);
  assert.deepEqual(m.up("Space"), [u("Space")]);
  assert.deepEqual(m.up("ControlLeft"), [u("ControlLeft")]);
  assert.deepEqual(m.up("MetaLeft"), [u("MetaLeft")]);
});

test("Ctrl+C on Windows is Command-C", () => {
  const m = new KeyMapper(true);
  assert.deepEqual(m.down("ControlLeft"), []);
  assert.deepEqual(m.down("KeyC"), [d("MetaLeft"), d("KeyC")]);
  assert.deepEqual(m.up("KeyC"), [u("KeyC")]);
  assert.deepEqual(m.down("KeyV"), [d("KeyV")], "Ctrl stays Command while held");
  assert.deepEqual(m.up("KeyV"), [u("KeyV")]);
  assert.deepEqual(m.up("ControlLeft"), [u("MetaLeft")]);
});

test("the right Ctrl is the right Command", () => {
  const m = new KeyMapper(true);
  m.down("ControlRight");
  assert.deepEqual(m.down("KeyZ"), [d("MetaRight"), d("KeyZ")]);
  m.up("KeyZ");
  assert.deepEqual(m.up("ControlRight"), [u("MetaRight")]);
});

test("Ctrl+Shift+Z is Command-Shift-Z: modifiers don't decide", () => {
  const m = new KeyMapper(true);
  m.down("ControlLeft");
  assert.deepEqual(m.down("ShiftLeft"), [d("ShiftLeft")]);
  assert.deepEqual(m.down("KeyZ"), [d("MetaLeft"), d("KeyZ")]);
});

test("Ctrl+Space is Control-Space", () => {
  const m = new KeyMapper(true);
  m.down("ControlLeft");
  assert.deepEqual(m.down("Space"), [d("ControlLeft"), d("Space")]);
  assert.deepEqual(m.down("Space"), [d("Space")], "a repeat");
  assert.deepEqual(m.up("Space"), [u("Space")]);
  assert.deepEqual(m.up("ControlLeft"), [u("ControlLeft")]);
});

test("Ctrl+Shift+Space is Control-Shift-Space", () => {
  const m = new KeyMapper(true);
  m.down("ControlLeft");
  m.down("ShiftLeft");
  assert.deepEqual(m.down("Space"), [d("ControlLeft"), d("Space")]);
});

test("Ctrl alone presses nothing, even repeating", () => {
  const m = new KeyMapper(true);
  assert.deepEqual(m.down("ControlLeft"), []);
  assert.deepEqual(m.down("ControlLeft"), []);
  assert.deepEqual(m.held(), ["ControlLeft"]);
  assert.deepEqual(m.up("ControlLeft"), []);
  assert.deepEqual(m.held(), []);
});

test("a Ctrl repeat after it was decided presses its device key again", () => {
  const m = new KeyMapper(true);
  m.down("ControlLeft");
  m.down("KeyA");
  assert.deepEqual(m.down("ControlLeft"), [d("MetaLeft")]);
});

test("the Windows key is Control", () => {
  const m = new KeyMapper(true);
  assert.deepEqual(m.down("MetaLeft"), [d("ControlLeft")]);
  assert.deepEqual(m.down("KeyE"), [d("KeyE")]);
  assert.deepEqual(m.up("KeyE"), [u("KeyE")]);
  assert.deepEqual(m.up("MetaLeft"), [u("ControlLeft")]);
  assert.deepEqual(m.down("MetaRight"), [d("ControlRight")]);
});

test("AltGr's added Control is dropped", () => {
  const m = new KeyMapper(true);
  assert.deepEqual(m.down("ControlLeft"), []);
  assert.deepEqual(m.down("AltRight", true), [d("AltRight")]);
  assert.deepEqual(m.down("KeyE"), [d("KeyE")]);
  assert.deepEqual(m.down("ControlLeft"), [], "its repeat");
  assert.deepEqual(m.up("KeyE"), [u("KeyE")]);
  assert.deepEqual(m.up("ControlLeft"), []);
  assert.deepEqual(m.up("AltRight"), [u("AltRight")]);
});

test("Ctrl with a plain right Alt (no AltGraph) stays Command", () => {
  const m = new KeyMapper(true);
  m.down("ControlLeft");
  assert.deepEqual(m.down("AltRight", false), [d("AltRight")]);
  assert.deepEqual(m.down("KeyK"), [d("MetaLeft"), d("KeyK")]);
});

test("an up with no down sends nothing", () => {
  const m = new KeyMapper(true);
  assert.deepEqual(m.up("KeyQ"), []);
});

test("releaseAll lets go of what the device has, and forgets the rest", () => {
  const m = new KeyMapper(true);
  m.down("ShiftLeft");
  m.down("ControlLeft");
  m.down("KeyA");
  m.down("ControlRight");
  assert.deepEqual(m.releaseAll(), [u("ShiftLeft"), u("MetaLeft"), u("KeyA")]);
  assert.deepEqual(m.held(), []);
  assert.deepEqual(m.up("ControlRight"), []);
});
