// A launch sheet (move to Applications, set up agents): a modal dialog over the dimmed window,
// shown with `showModal` while `open`.

import { useEffect, useRef, type FormEvent, type ReactNode } from "react";

export function Sheet(props: { id: string; titleId: string; open: boolean; onSubmit: () => void; onCancel?: () => void; children: ReactNode }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const { open } = props;
  useEffect(() => {
    const d = dialog.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    else if (!open && d.open) d.close();
  }, [open]);
  return (
    <dialog
      id={props.id}
      ref={dialog}
      className="sheet"
      aria-labelledby={props.titleId}
      // Escape closes the dialog by itself: tell the owner, so it shows closed too.
      onClose={props.onCancel}
    >
      <form
        method="dialog"
        onSubmit={(e: FormEvent) => {
          e.preventDefault();
          props.onSubmit();
        }}
      >
        {props.children}
      </form>
    </dialog>
  );
}
