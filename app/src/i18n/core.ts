// The viewer's localization, without the DOM or the catalogs, so the unit tests (i18n.test.ts)
// run it without a page: which language to use, and how a message is filled in. i18n.ts adds
// the catalogs (locales/*.json) and the page; react.ts lets components follow the language.

/** The languages the viewer has catalogs for, as BCP 47 tags; `en` is the source. */
export const LANGUAGES = ["en", "ko", "ja", "zh-Hans", "zh-Hant", "es", "fr", "de", "pt-BR", "it"] as const;
export type Language = (typeof LANGUAGES)[number];

/** Each language in its own words, for the language picker: never translated. */
export const LANGUAGE_NAMES: Record<Language, string> = {
  en: "English",
  ko: "한국어",
  ja: "日本語",
  "zh-Hans": "简体中文",
  "zh-Hant": "繁體中文",
  es: "Español",
  fr: "Français",
  de: "Deutsch",
  "pt-BR": "Português (Brasil)",
  it: "Italiano",
};

/** One message: text, or text per plural category (Intl.PluralRules), chosen by `count`. */
export type Message = string | Partial<Record<Intl.LDMLPluralRule, string>>;
export type Catalog = Record<string, Message>;
export type Vars = Record<string, string | number>;

/** The catalog for a language tag from the OS or the browser, if there is one. Chinese goes by
 * script (zh-TW and zh-HK are Traditional, zh and zh-CN Simplified); any Portuguese gets the
 * Brazilian catalog, the only one. */
export function matchLanguage(tag: string): Language | undefined {
  let loc: Intl.Locale;
  try {
    loc = new Intl.Locale(tag).maximize();
  } catch {
    return undefined;
  }
  if (loc.language === "zh") return loc.script === "Hant" ? "zh-Hant" : "zh-Hans";
  if (loc.language === "pt") return "pt-BR";
  return LANGUAGES.find((l) => l === loc.language);
}

/** The first of `tags` (most preferred first, as navigator.languages) with a catalog, else English. */
export function resolveLanguage(tags: readonly string[]): Language {
  for (const tag of tags) {
    const l = matchLanguage(tag);
    if (l) return l;
  }
  return "en";
}

/** The text of `m` in `lang` with `{name}` placeholders filled from `vars`. Numbers are written
 * the language's way; a plural message picks its form by `vars.count`, falling back to `other`. */
export function format(m: Message, lang: Language, vars: Vars = {}): string {
  let text: string;
  if (typeof m === "string") text = m;
  else {
    const category = new Intl.PluralRules(lang).select(Number(vars.count ?? 0));
    text = m[category] ?? m.other ?? "";
  }
  return text.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const v = vars[name];
    if (v === undefined) return whole;
    return typeof v === "number" ? new Intl.NumberFormat(lang).format(v) : v;
  });
}

/** The placeholder names a message uses, in every plural form. */
export function placeholders(m: Message): Set<string> {
  const texts = typeof m === "string" ? [m] : Object.values(m);
  const names = new Set<string>();
  for (const text of texts) for (const match of (text ?? "").matchAll(/\{(\w+)\}/g)) names.add(match[1]);
  return names;
}
