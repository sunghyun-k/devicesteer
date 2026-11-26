// The viewer's localization: the catalogs (locales/*.json, English the source), `t()`, and the
// language setting (Settings › General; the OS or browser languages by default). Components read
// messages through `useT()` (react.ts), which renders them again when the language changes.
//
// A new string a person sees goes through `t()` with a key in every catalog in the same change
// (AGENTS.md, "Language"); i18n.test.ts checks that every catalog has every key with the same
// placeholders. Not translated: what agents read (CLI output, JSON, skill.md, the daemon's error
// texts) and messages from the daemon or the core that the viewer shows as they are.

import { emit, listen } from "@tauri-apps/api/event";
import { LANGUAGES, format, resolveLanguage, type Catalog, type Language, type Vars } from "./core";
import en from "../locales/en.json";
import ko from "../locales/ko.json";
import ja from "../locales/ja.json";
import zhHans from "../locales/zh-Hans.json";
import zhHant from "../locales/zh-Hant.json";
import es from "../locales/es.json";
import fr from "../locales/fr.json";
import de from "../locales/de.json";
import ptBR from "../locales/pt-BR.json";
import it from "../locales/it.json";

export { LANGUAGES, LANGUAGE_NAMES, type Language, type Vars } from "./core";

const CATALOGS: Record<Language, Catalog> = { en, ko, ja, "zh-Hans": zhHans, "zh-Hant": zhHant, es, fr, de, "pt-BR": ptBR, it };

/** Every message key, from the English catalog. */
export type Key = keyof typeof en;

/** `t`, as helpers that write messages take it: components pass the one from `useT()`, so React
 * Compiler sees the language as an input of what they compute; other modules pass `t`. */
export type Translate = (key: Key, vars?: Vars) => string;

/** Text the page shows: written as it is (the daemon's or the core's words, in English), or a
 * message written in whatever language is current when it is shown. Kept in state as a message,
 * so a language change says it again in the new language. */
export type Text = string | { key: Key; vars?: Vars };

/** A message to show later in the current language. */
export const msg = (key: Key, vars?: Vars): Text => ({ key, vars });

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
let setting = languageSetting();
document.documentElement.lang = lang;

/** The language the viewer shows now. */
export const language = (): Language => lang;

// Messages that name the computer ("this Mac") or its ⌘ shortcuts have a `<key>@windows` sibling
// that `t()` takes in their place on Windows, so callers don't choose.
const WINDOWS = /Win/.test(navigator.platform);

/** The message `key` in the current language, `{name}` placeholders filled from `vars`. */
export function t(key: Key, vars?: Vars): string {
  const k = WINDOWS && hasKey(`${key}@windows`) ? (`${key}@windows` as Key) : key;
  return format(CATALOGS[lang][k] ?? en[k], lang, vars);
}

/** `text` as it shows now: a message in the current language, or the string itself. */
export const say = (text: Text): string => (typeof text === "string" ? text : t(text.key, text.vars));

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

/** The language setting as last read: what the picker shows. */
export const currentLanguageSetting = (): Language | "system" => setting;

const listeners = new Set<() => void>();

/** Runs `redraw` whenever the language or its setting changes; returns the unsubscribe. React
 * components use `useT()` instead. */
export function onLanguageChange(redraw: () => void): () => void {
  listeners.add(redraw);
  return () => listeners.delete(redraw);
}

/** Picks the language again (the setting, or the OS's languages, changed) and redraws. */
function refresh(): void {
  const next = chosen();
  const nextSetting = languageSetting();
  if (next === lang && nextSetting === setting) return;
  lang = next;
  setting = nextSetting;
  document.documentElement.lang = lang;
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

/** Follows changes made elsewhere: the other window changed the setting, the OS's languages
 * changed, another tab of the mock did. Called once by each page's entry, after the mock (if any)
 * is installed, so the Tauri event reaches it. */
export function followLanguageChanges(): void {
  void listen("language-changed", refresh).catch(() => {});
  window.addEventListener("languagechange", refresh);
  window.addEventListener("storage", (e) => {
    if (e.key === SETTING) refresh();
  });
}
