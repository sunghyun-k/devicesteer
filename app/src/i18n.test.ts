// Unit tests for the viewer's localization: every catalog has every message of the English one,
// with the same placeholders and plural forms its language uses, and `t()`'s logic (i18n-core.ts)
// picks languages, fills placeholders and chooses plural forms. Run with `npm test` (plain Node,
// which runs TypeScript itself; the catalogs are read as files, so no bundler is involved).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { LANGUAGES, format, matchLanguage, placeholders, resolveLanguage, type Catalog, type Language } from "./i18n-core.ts";

const catalog = (lang: Language): Catalog => JSON.parse(readFileSync(new URL(`./locales/${lang}.json`, import.meta.url), "utf8"));
const en = catalog("en");

for (const lang of LANGUAGES.filter((l) => l !== "en")) {
  test(`${lang} has every message, with the same placeholders`, () => {
    const c = catalog(lang);
    assert.deepEqual(Object.keys(c), Object.keys(en), "same keys in the same order as en.json");
    const categories = new Set(new Intl.PluralRules(lang).resolvedOptions().pluralCategories);
    for (const [key, source] of Object.entries(en)) {
      const m = c[key];
      assert.equal(typeof m, typeof source, `${key}: a ${typeof source === "string" ? "string" : "plural object"}`);
      assert.deepEqual([...placeholders(m)].sort(), [...placeholders(source)].sort(), `${key}: placeholders`);
      if (typeof m === "string") {
        assert.ok(m.trim(), `${key}: empty`);
        continue;
      }
      for (const [form, text] of Object.entries(m)) {
        assert.ok(categories.has(form as Intl.LDMLPluralRule), `${key}: ${lang} has no plural form "${form}"`);
        assert.deepEqual([...placeholders(text ?? "")].sort(), [...placeholders(source)].sort(), `${key}.${form}: placeholders`);
      }
      for (const form of categories) {
        // `many` is for large numbers only in these languages; `other` covers it.
        if (form !== "many") assert.ok(m[form], `${key}: missing plural form "${form}"`);
      }
    }
  });
}

test("plural messages in English have one and other", () => {
  for (const [key, m] of Object.entries(en)) {
    if (typeof m !== "string") assert.ok(m.one && m.other, key);
  }
});

test("menu labels name the app only through {app}", () => {
  for (const lang of LANGUAGES) {
    for (const [key, m] of Object.entries(catalog(lang))) {
      if (key.startsWith("menu.")) assert.ok(!String(m).includes("DeviceSteer"), `${lang} ${key}`);
    }
  }
});

test("matches OS and browser language tags", () => {
  assert.equal(matchLanguage("en-US"), "en");
  assert.equal(matchLanguage("ko-KR"), "ko");
  assert.equal(matchLanguage("ja"), "ja");
  assert.equal(matchLanguage("zh-CN"), "zh-Hans");
  assert.equal(matchLanguage("zh"), "zh-Hans");
  assert.equal(matchLanguage("zh-Hans-SG"), "zh-Hans");
  assert.equal(matchLanguage("zh-TW"), "zh-Hant");
  assert.equal(matchLanguage("zh-HK"), "zh-Hant");
  assert.equal(matchLanguage("pt-PT"), "pt-BR");
  assert.equal(matchLanguage("de-CH"), "de");
  assert.equal(matchLanguage("nl-NL"), undefined);
  assert.equal(matchLanguage("not a tag!"), undefined);
});

test("the first supported language wins, else English", () => {
  assert.equal(resolveLanguage(["nl-NL", "fr-CA", "de"]), "fr");
  assert.equal(resolveLanguage(["nl", "sv"]), "en");
  assert.equal(resolveLanguage([]), "en");
});

test("format fills placeholders and picks plural forms", () => {
  assert.equal(format("Hello {name}", "en", { name: "A" }), "Hello A");
  assert.equal(format("{n} of {m}", "de", { n: 1234, m: 2 }), "1.234 of 2");
  assert.equal(format("{missing} stays", "en"), "{missing} stays");
  const m = { one: "{count} frame", other: "{count} frames" };
  assert.equal(format(m, "en", { count: 1 }), "1 frame");
  assert.equal(format(m, "en", { count: 3 }), "3 frames");
  assert.equal(format({ other: "{count}개" }, "ko", { count: 1 }), "1개");
  // A form the message lacks falls back to `other`.
  assert.equal(format({ one: "un", other: "autres" }, "fr", { count: 1_000_000 }), "autres");
});
