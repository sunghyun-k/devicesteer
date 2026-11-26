// Local settings of the windows (panels shown, last device, sheets offered): conveniences only,
// so storage that is missing or throws just means defaults. Both windows share the origin, so
// the keys are shared too.

export const prefs = {
  get(key: string): string | null {
    try {
      return localStorage.getItem(`devicesteer.${key}`);
    } catch {
      return null;
    }
  },
  set(key: string, value: string): void {
    try {
      localStorage.setItem(`devicesteer.${key}`, value);
    } catch {
      // Not kept; the default comes back next time.
    }
  },
};
