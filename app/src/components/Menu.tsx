// A button that opens a menu under it: Escape and a click elsewhere close it, the arrow keys move
// through its items, picking one closes it. Which menu is open is in the store, so the keys don't
// go to the device while one is (input/keyboard.ts).

import { useEffect, useRef, type ReactNode } from "react";
import { getViewer, setViewer, useViewer, type ViewerState } from "../store/viewer";
import { Icon } from "./Icon";
import s from "./Menu.module.css";

type MenuName = NonNullable<ViewerState["openMenu"]>;

export const closeMenu = () => setViewer({ openMenu: null });

export function Menu(props: {
  name: MenuName;
  /** The button's id, and the menu's (`aria-controls`). */
  id: string;
  menuId: string;
  icon: string;
  label: string;
  title: string;
  /** Extra attributes for the button (state shown by its color). */
  buttonProps?: Record<string, string | undefined>;
  children: ReactNode;
}) {
  const open = useViewer((v) => v.openMenu === props.name);
  const anchor = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    list.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    const away = (e: PointerEvent) => {
      if (!anchor.current?.contains(e.target as Node)) closeMenu();
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [open]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const items = [...(list.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") {
      closeMenu();
      button.current?.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
    }
  };

  return (
    <div className={s.anchor} ref={anchor}>
      <button
        id={props.id}
        ref={button}
        className="icon-btn"
        type="button"
        aria-label={props.label}
        title={props.title}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={props.menuId}
        onClick={() => setViewer({ openMenu: getViewer().openMenu === props.name ? null : props.name })}
        {...props.buttonProps}
      >
        <Icon name={props.icon} />
      </button>
      <div
        id={props.menuId}
        ref={list}
        className={s.menu}
        role="menu"
        hidden={!open}
        onKeyDown={onKeyDown}
        onClick={(e) => {
          if ((e.target as Element).closest("button")) closeMenu();
        }}
      >
        {props.children}
      </div>
    </div>
  );
}

/** A row of a menu. `checked` makes it a checkable row, its check at `checkAt`. */
export function MenuItem(props: {
  id: string;
  icon: string;
  label: string;
  title?: string;
  shortcut?: string;
  checked?: boolean;
  /** Where a checkable row's check goes: before the icon, or at the trailing edge. */
  checkAt?: "start" | "end";
  disabled?: boolean;
  onSelect: () => void;
}) {
  const check = props.checked !== undefined ? <Icon name="i-check" className={`icon ${s.check}`} /> : null;
  return (
    <button
      id={props.id}
      role={props.checked !== undefined ? "menuitemcheckbox" : "menuitem"}
      aria-checked={props.checked}
      type="button"
      title={props.title}
      disabled={props.disabled}
      onClick={props.onSelect}
    >
      {props.checkAt === "start" ? check : <Icon name={props.icon} />}
      <span>{props.label}</span>
      {props.shortcut ? <kbd>{props.shortcut}</kbd> : null}
      {props.checkAt === "end" ? check : null}
    </button>
  );
}

export const MenuSeparator = () => <div className={s.sep} role="separator" />;
