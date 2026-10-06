import { Dialog } from '@uwusuite/design';
import { useEffect, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

type ModalProps = {
  title: string;
  /** Security warnings get their own look, so they never blend in with routine dialogs. */
  tone?: 'default' | 'warning';
  /**
   * `default` for questions and short forms, `small` for a yes or no, `wide`
   * for Settings (a section list next to the content) and the setup wizard,
   * which keeps its title and buttons in place and scrolls in between.
   */
  size?: 'default' | 'small' | 'wide' | 'wizard';
  onCancel: () => void;
  children: ReactNode;
  footer?: ReactNode;
  /** Extra classes on the <dialog>, for a dialog that needs a fixed height (Settings). */
  className?: string;
  /** Classes for the body box instead of the default padding and gap. */
  bodyClassName?: string;
};

const WIDTH = { small: 'sm', default: 'md', wide: 'lg', wizard: 'lg' } as const;

/**
 * A dialog: @uwusuite/design's `Dialog` (a native <dialog>, so the page behind
 * it is inert, Tab stays inside and the top one gets Escape), with what UwUSSH
 * adds on top:
 *
 * - Escape and the dialog's own buttons close it, a click beside it doesn't:
 *   closing by accident threw away whatever was typed into it.
 * - Focus lands where it is safe. An explicit [data-autofocus] wins —
 *   warnings point it at the safe choice. Otherwise the first field, or the
 *   first button not marked [data-secondary]: buttons that act on something
 *   risky carry it, so Enter can never trigger them by accident. When nothing
 *   should be focused — the host key dialog deliberately gives neither button
 *   default focus, so Enter cannot trust a key by accident — the dialog itself
 *   takes it. Leaving focus behind let Enter activate whatever was focused in
 *   the background: in the end-to-end test that was the host row, and it
 *   started a second connection.
 * - Focus goes back where it was on close.
 *
 * Rendered straight into <body>: a dialog opened from inside another one (the
 * export from Settings) must not depend on the box it was opened from.
 */
export function Modal({
  title,
  tone = 'default',
  size = 'default',
  onCancel,
  children,
  footer,
  className,
  bodyClassName,
}: ModalProps) {
  const bodyRef = useRef<HTMLDivElement>(null);
  // Escape calls the latest onCancel, not the one from when the dialog
  // opened: a form that asks before closing only knows once something changed.
  const cancelRef = useRef(onCancel);
  cancelRef.current = onCancel;

  useEffect(() => {
    const body = bodyRef.current;
    const dialog = body?.closest('dialog');
    if (!body || !dialog) return;
    const previous = document.activeElement as HTMLElement | null;
    const first =
      dialog.querySelector<HTMLElement>('[data-autofocus]') ??
      body.querySelector<HTMLElement>('input, button:not([data-secondary]), textarea, select') ??
      dialog.querySelector<HTMLElement>('footer button:not([data-secondary])');
    if (first) {
      first.focus();
    } else {
      dialog.tabIndex = -1;
      dialog.focus();
    }
    return () => {
      if (previous?.isConnected) previous.focus();
    };
  }, []);

  return createPortal(
    <Dialog
      open
      onClose={() => cancelRef.current()}
      title={title}
      tone={tone}
      width={WIDTH[size]}
      footer={footer}
      closeOnOutsideClick={false}
      className={['uwu-modal', tone === 'warning' && 'uwu-modal-warning', className]
        .filter(Boolean)
        .join(' ')}
    >
      <div
        ref={bodyRef}
        className={bodyClassName ?? 'modal-body grid gap-3 px-6 pt-1 pb-5 phone:px-4'}
        data-size={size}
      >
        {children}
      </div>
    </Dialog>,
    document.body,
  );
}
