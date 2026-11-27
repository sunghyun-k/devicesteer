// The Keyboard button and its menu, like Xcode's Device Hub's: the button's colour says whether
// keys go to the device (keyboard.ts).

import { Menu, MenuItem, MenuSeparator } from "../../components/Menu";
import { useT } from "../../i18n/react";
import { humanHasDevice, useViewer } from "../../store/viewer";
import { getClipboard, keyboardButtonState, sendClipboard, toggleForwarding, toggleSharedClipboard, toggleSoftwareKeyboard } from "./keyboard";

const STATE_KEYS = { on: "keyboard.stateOn", connecting: "keyboard.stateConnecting", paused: "keyboard.statePaused", off: "keyboard.label" } as const;

export function KeyboardMenu({ className, hidden }: { className: string; hidden: boolean }) {
  const t = useT();
  const forwarding = useViewer((v) => v.keyboard.forwarding);
  const hardware = useViewer((v) => v.keyboard.hardware);
  const blocked = useViewer((v) => !humanHasDevice(v));
  const state = useViewer(keyboardButtonState);
  const shared = useViewer((v) => v.sharedClipboard);
  const sharedUnsupported = useViewer((v) => v.sharedClipboardUnsupported);
  const label = t(STATE_KEYS[state]);
  return (
    <div className={className} id="keyboard-group" hidden={hidden}>
      <Menu name="keyboard" id="keyboard" menuId="keyboard-menu" icon="i-keyboard" label={label} title={label} buttonProps={{ "data-forwarding": state }}>
        <MenuItem
          id="kb-forward"
          icon="i-check"
          checkAt="start"
          checked={forwarding}
          label={t("keyboard.forward")}
          title={t("keyboard.forwardHelp")}
          shortcut={t("keyboard.forwardKey")}
          onSelect={toggleForwarding}
        />
        <MenuItem
          id="kb-software"
          icon="i-keyboard-down"
          label={t("keyboard.software")}
          title={blocked ? t("session.blockedHint") : t("keyboard.softwareHelp")}
          shortcut={t("keyboard.softwareKey")}
          disabled={!hardware || blocked}
          onSelect={toggleSoftwareKeyboard}
        />
        <MenuSeparator />
        <MenuItem
          id="kb-clip-shared"
          icon="i-clip-sync"
          checkAt="end"
          checked={shared}
          label={t("keyboard.shared")}
          // No way to see password managers' private items on this platform (shared_clip.rs).
          title={sharedUnsupported ? t("keyboard.sharedMacOnly") : t("keyboard.sharedHelp")}
          disabled={sharedUnsupported}
          onSelect={toggleSharedClipboard}
        />
        <MenuItem id="kb-clip-get" icon="i-clip-get" label={t("keyboard.get")} title={t("keyboard.getHelp")} shortcut={t("keyboard.getKey")} onSelect={() => void getClipboard()} />
        <MenuItem id="kb-clip-send" icon="i-clip-send" label={t("keyboard.send")} title={t("keyboard.sendHelp")} shortcut={t("keyboard.sendKey")} onSelect={() => void sendClipboard()} />
      </Menu>
    </div>
  );
}
