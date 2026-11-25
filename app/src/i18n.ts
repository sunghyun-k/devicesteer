// The viewer's localization: the catalogs (locales/*.json, English the source), `t()`, the
// language setting (Settings › General; the OS or browser languages by default), and the
// `data-i18n*` attributes that translate the static HTML.
//
// A new string a person sees goes through `t()` with a key in every catalog in the same change
// (AGENTS.md, "Language"); i18n.test.ts checks that every catalog has every key with the same
// placeholders. Not translated: what agents read (CLI output, JSON, skill.md, the daemon's error
// texts) and messages from the daemon or the core that the viewer shows as they are.

import { emit, listen } from "@tauri-apps/api/event";
import { LANGUAGES, format, resolveLanguage, type Catalog, type Language, type Vars } from "./i18n-core";
import en from "./locales/en.json";
import ko from "./locales/ko.json";
import ja from "./locales/ja.json";
import zhHans from "./locales/zh-Hans.json";
import zhHant from "./locales/zh-Hant.json";
import es from "./locales/es.json";
import fr from "./locales/fr.json";
import de from "./locales/de.json";
import ptBR from "./locales/pt-BR.json";
import it from "./locales/it.json";

export { LANGUAGES, LANGUAGE_NAMES, type Language } from "./i18n-core";

const CATALOGS: Record<Language, Catalog> = { en, ko, ja, "zh-Hans": zhHans, "zh-Hant": zhHant, es, fr, de, "pt-BR": ptBR, it };

/** Every message key, from the English catalog. */
export type Key = keyof typeof en;

const SETTING = "devicesteer.language";

/** The language setting: one of LANGUAGES, or `system` to follow the OS or browser. */
export function languageSetting(): Language | "system" {
  try {
    const v = localStorage.getItem(SETTING);
    return LANGUAGES.find((l) => l === v) ?? "system";
  } catch {
    return "system";
  }
}

function chosen(): Language {
  const s = languageSetting();
  if (s !== "system") return s;
  return resolveLanguage(navigator.languages?.length ? navigator.languages : [navigator.language]);
}

let lang: Language = chosen();
document.documentElement.lang = lang;

/** The language the viewer shows now. */
export const language = (): Language => lang;

/** The message `key` in the current language, `{name}` placeholders filled from `vars`. */
export function t(key: Key, vars?: Vars): string {
  return format(CATALOGS[lang][key] ?? en[key], lang, vars);
}

/** A number written the current language's way. */
export function num(n: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(lang, options).format(n);
}

/** Whether `key` is a message key: for codes that come from the shell (`errorText`). */
export const hasKey = (key: string): key is Key => key in en;

/** What to show for an error from the Tauri shell. One it wrote for the person carries a `code`
 * with a message under `error.<code>` (its other fields fill the placeholders); anything else,
 * from the daemon, the core or the OS, is shown in English as it came. */
export function errorText(e: unknown): string {
  if (e && typeof e === "object") {
    const o = e as Record<string, unknown>;
    const key = `error.${String(o.code)}`;
    if (typeof o.code === "string" && hasKey(key)) {
      const vars: Vars = {};
      for (const [k, v] of Object.entries(o)) if (typeof v === "string" || typeof v === "number") vars[k] = v;
      return t(key, vars);
    }
    if (typeof o.message === "string") return o.message;
  }
  return String(e);
}

/** Translates the static HTML under `root`: `data-i18n` sets the text, `data-i18n-title`,
 * `data-i18n-aria-label` and `data-i18n-placeholder` those attributes. */
export function localize(root: ParentNode = document): void {
  for (const e of root.querySelectorAll<HTMLElement>("[data-i18n]")) e.textContent = t(e.dataset.i18n as Key);
  for (const [data, attr] of [
    ["i18nTitle", "title"],
    ["i18nAriaLabel", "aria-label"],
    ["i18nPlaceholder", "placeholder"],
  ] as const) {
    const sel = `[data-${data.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}]`;
    for (const e of root.querySelectorAll<HTMLElement>(sel)) e.setAttribute(attr, t(e.dataset[data] as Key));
  }
}

const listeners: (() => void)[] = [];

/** Runs `redraw` whenever the language changes, after the static HTML is translated again. */
export function onLanguageChange(redraw: () => void): void {
  listeners.push(redraw);
}

/** Picks the language again (the setting, or the OS's languages, changed) and redraws. */
function refresh(): void {
  const next = chosen();
  if (next === lang) return;
  lang = next;
  document.documentElement.lang = lang;
  localize();
  for (const redraw of listeners) redraw();
}

/** Changes the language setting here and in the app's other windows. */
export function setLanguageSetting(v: Language | "system"): void {
  try {
    if (v === "system") localStorage.removeItem(SETTING);
    else localStorage.setItem(SETTING, v);
  } catch {
    // Not kept: the language changes for now and goes back at the next launch.
  }
  refresh();
  // The windows share the storage but not its events: tell the other one.
  void emit("language-changed").catch(() => {});
}

// The other window changed the setting; the OS's languages changed; another tab of the mock did.
void listen("language-changed", refresh).catch(() => {});
window.addEventListener("languagechange", refresh);
window.addEventListener("storage", (e) => {
  if (e.key === SETTING) refresh();
});
