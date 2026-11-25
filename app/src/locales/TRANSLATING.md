# Translating the viewer

`en.json` is the source. Every other file here has exactly the same keys, in the same order,
with the same `{placeholders}`; `npm test` checks that. A new or changed English string is
translated in every file in the same change (AGENTS.md, "Language").

## The format

- Flat keys; a value is a string, or for a count an object of plural forms by
  [Intl.PluralRules](https://developer.mozilla.org/docs/Web/JavaScript/Reference/Global_Objects/Intl/PluralRules)
  category (`one`, `other`, ...). Give the forms the language uses: Korean, Japanese and
  Chinese only `other`; the others `one` and `other` (`many`, which French, Spanish, Italian
  and Portuguese use for millions, is optional: `other` stands in).
- Keep `{name}` placeholders exactly; move them where the sentence needs them. Numbers in them
  are formatted for the language by the code.
- Keep `DeviceSteer`, `iPhone`, `iPad`, `iOS`, `Wi-Fi`, `USB`, `VoiceOver`, `Liquid Glass`,
  `Xcode`, `macOS`, `PATH`, `Finder` (but Simplified Chinese calls it 访达), key symbols
  (⌘K, ⌥⌘K, Control-Space) and `›` path separators as they are.
- A key ending in `@windows` replaces the key before it on Windows: the same message for a PC
  ("this PC" for "this Mac", `Ctrl+` for `⌘`; German keyboards say `Strg+`). Use the word the
  language's other PC strings use (`prefs.notFoundPc`).
- `menu.*` are the Mac app menu: use the exact titles macOS uses in that language (About,
  Services, Hide, Quit, Edit, ...). `common.listSeparator` is the language's list comma.

## Words

Use one term throughout, as Apple writes it in that language where Apple has one:

- **session** (agent session): the period an AI agent controls the device.
- **take over** / **hand back**: the person takes the device from the agent, and gives it back.
- **pair / pairing**: as iOS says for pairing with a computer.
- **Developer Mode**: Apple's name in each language, e.g. 개발자 모드, デベロッパモード,
  开发者模式, 開發者模式, Modo de desarrollador, Mode développeur, Entwicklermodus,
  Modo de Desenvolvedor, Modalità sviluppatore.
- **agent** (coding agent): the AI tool such as Claude Code.
- **Apple Devices** (the Windows app from the Microsoft Store): its name in that language's
  Store, e.g. Apple 기기, Appleデバイス, Apple 设备, Apple 裝置, Dispositivos Apple, Appareils
  Apple, Apple Geräte, Dispositivi Apple. `iTunes` and `Microsoft Store` stay as they are.

## What the person must find on the device

These name things on the iPhone or iPad, so they must match iOS's wording in that language:

- Settings › General › Software Update
- Settings › Privacy & Security › Developer Mode, its switch, and the alerts' buttons
  Restart and Turn On
- The alert "Trust This Computer?" and its button Trust
- In the Developer Mode screen (iOS 27): the section Other Devices and the button
  Pair with DeviceSteer, then Pair
