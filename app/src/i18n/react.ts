// `t()` for components: a component that reads messages through `useT()` renders again when the
// language changes (Settings › General, or the OS's languages).

import { useSyncExternalStore } from "react";
import { currentLanguageSetting, language, num, onLanguageChange, say, t, type Language } from "./i18n";

/** A `t` per language: a new function when the language changes, so React Compiler's memoized
 * output that used the old one is thrown away. */
const translators = new Map<Language, typeof t & { say: typeof say; num: typeof num; lang: Language }>();

function translator(lang: Language) {
  let fn = translators.get(lang);
  if (!fn) {
    fn = Object.assign((...args: Parameters<typeof t>) => t(...args), { say: (x: Parameters<typeof say>[0]) => say(x), num: (...args: Parameters<typeof num>) => num(...args), lang });
    translators.set(lang, fn);
  }
  return fn;
}

/** The current language. */
export function useLanguage(): Language {
  return useSyncExternalStore(onLanguageChange, language);
}

/** `t`, with `t.say(text)` for a `Text`, `t.num` and `t.lang`, for the current language. */
export function useT() {
  return translator(useLanguage());
}

/** The language setting: one of the languages, or `system`. */
export function useLanguageSetting(): Language | "system" {
  return useSyncExternalStore(onLanguageChange, currentLanguageSetting);
}
