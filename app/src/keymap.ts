// Which device key each computer key presses while "Type from PC/Mac Keyboard" is on. Pure
// logic, kept out of main.ts so `npm test` can run it (keymap.test.ts).
//
// On a Mac every key is itself: ⌘ is Command, Control is Control. On Windows the keys follow an
// iPad with a PC keyboard whose Control and Command are swapped in Settings › General › Keyboard
// › Hardware Keyboard › Modifier Keys, so the PC's habits work on the device:
//
// - Ctrl is Command: Ctrl+C, Ctrl+V, Ctrl+X, Ctrl+A, Ctrl+Z copy, paste, cut, select all and undo.
// - The Windows key is Control, for the combinations Windows lets the page see (it keeps most
//   Win+letter ones for itself, and Win+Space switches the PC's own input source). It can't be
//   Command: alone it opens Start (PITFALLS #67).
// - Ctrl+Space is Control-Space, which switches the device's input source: with Win+Space taken
//   by Windows, nothing else could press it (PITFALLS #67). Command-Space (Spotlight) is given
//   up for it.
//
// For that last one a Ctrl key goes down on the device only when the next key that isn't a
// modifier shows which it means: Space makes it Control, anything else Command. A Ctrl pressed
// and let go alone presses nothing.
//
// AltGr on a European layout reaches the page as ControlLeft and AltRight (Windows adds the
// Control; Chromium sets the AltGraph modifier): the ControlLeft is dropped, or AltGr+E would be
// Command-Option-E on the device instead of Option-E.

export type KeyStep = { code: string; down: boolean };

const isCtrl = (code: string): boolean => code === "ControlLeft" || code === "ControlRight";
const MODIFIERS = new Set(["ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight", "ControlLeft", "ControlRight", "CapsLock", "Fn"]);

/** Maps the computer's key downs and ups (by DOM `code`) to the device's, and remembers what it
 * sent so each is let go again. `pc`: the Windows mapping above; otherwise every key is itself. */
export class KeyMapper {
  /** Keys held on the computer whose down went to the device, with the device key it pressed. */
  private sent = new Map<string, string>();
  /** Ctrl keys held whose meaning no key has shown yet. */
  private pending = new Set<string>();
  /** Control keys Windows added for AltGr: their up sends nothing either. */
  private dropped = new Set<string>();

  private readonly pc: boolean;

  constructor(pc: boolean) {
    this.pc = pc;
  }

  /** Computer keys held, whether or not the device has them yet. */
  held(): string[] {
    return [...this.sent.keys(), ...this.pending, ...this.dropped];
  }

  /** A key went down (or repeated). `altGraph`: the event's AltGraph modifier state. */
  down(code: string, altGraph = false): KeyStep[] {
    if (!this.pc) return this.press(code, code);
    if (this.dropped.has(code)) return [];
    if (isCtrl(code)) {
      const as = this.sent.get(code);
      if (as) return [{ code: as, down: true }];
      this.pending.add(code);
      return [];
    }
    if (code === "AltRight" && altGraph && this.pending.delete("ControlLeft")) this.dropped.add("ControlLeft");
    const steps: KeyStep[] = [];
    if (!MODIFIERS.has(code)) {
      const ctrlIs = code === "Space" ? "Control" : "Meta";
      for (const c of this.pending) steps.push(...this.press(c, ctrlIs + (c === "ControlLeft" ? "Left" : "Right")));
      this.pending.clear();
    }
    const as = code === "MetaLeft" ? "ControlLeft" : code === "MetaRight" ? "ControlRight" : code;
    return [...steps, ...this.press(code, as)];
  }

  /** A key came up. */
  up(code: string): KeyStep[] {
    if (this.pending.delete(code) || this.dropped.delete(code)) return [];
    const as = this.sent.get(code);
    if (as === undefined) return [];
    this.sent.delete(code);
    return [{ code: as, down: false }];
  }

  /** Lets go of everything, for a lost focus or forwarding turned off. */
  releaseAll(): KeyStep[] {
    const steps = [...this.sent.values()].map((code) => ({ code, down: false }));
    this.sent.clear();
    this.pending.clear();
    this.dropped.clear();
    return steps;
  }

  private press(code: string, as: string): KeyStep[] {
    this.sent.set(code, as);
    return [{ code: as, down: true }];
  }
}
