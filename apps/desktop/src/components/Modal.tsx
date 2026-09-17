import { useEffect, useRef, type ReactNode } from 'react';

type ModalProps = {
  title: string;
  /** Security warnings get their own look, so they never blend in with routine dialogs. */
  tone?: 'default' | 'warning';
  /** Settings need room for a section list next to the content. */
  size?: 'default' | 'wide';
  onCancel: () => void;
  children: ReactNode;
  footer?: ReactNode;
};

const FOCUSABLE = 'input, button, textarea, select, [href], [tabindex]:not([tabindex="-1"])';

/**
 * A dialog. Escape and a click on the backdrop cancel; focus moves into the
 * dialog on open, stays inside it while it is open, and goes back where it was
 * on close.
 *
 * Focus always lands somewhere inside. When nothing should be focused — the
 * host key dialog deliberately gives neither button default focus, so Enter
 * cannot trust a key by accident — the dialog itself takes it. Leaving focus
 * behind let Enter activate whatever was focused in the background: in the
 * end-to-end test that was the host row, and it started a second connection.
 */
export function Modal({
  title,
  tone = 'default',
  size = 'default',
  onCancel,
  children,
  footer,
}: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previous = document.activeElement as HTMLElement | null;

    // An explicit [data-autofocus] wins — warnings point it at the safe choice.
    // Otherwise the first field, or the first button not marked secondary.
    // Buttons that act on something risky carry data-secondary, so Enter can
    // never trigger them by accident.
    const first =
      dialog.querySelector<HTMLElement>('[data-autofocus]') ??
      dialog.querySelector<HTMLElement>('input, button:not([data-secondary]), textarea, select') ??
      dialog;
    first.focus();

    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCancel();
        return;
      }
      if (event.key !== 'Tab') return;

      // Keep Tab inside the dialog.
      const focusable = [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
        (el) => !el.hasAttribute('disabled') && !el.hidden,
      );
      if (focusable.length === 0) {
        event.preventDefault();
        return;
      }
      const firstEl = focusable[0]!;
      const lastEl = focusable[focusable.length - 1]!;
      const current = document.activeElement;
      if (event.shiftKey && (current === firstEl || current === dialog)) {
        event.preventDefault();
        lastEl.focus();
      } else if (!event.shiftKey && current === lastEl) {
        event.preventDefault();
        firstEl.focus();
      }
    };

    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
      previous?.focus();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onCancel()}>
      <div
        ref={dialogRef}
        className="modal"
        data-tone={tone}
        data-size={size}
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
        tabIndex={-1}
      >
        <h2 id="modal-title" className="modal-title">
          {title}
        </h2>
        <div className="modal-body">{children}</div>
        {footer && <div className="modal-footer">{footer}</div>}
      </div>
    </div>
  );
}
