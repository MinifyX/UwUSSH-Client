import { cx, Icon } from '@uwusuite/design';
import type { LucideIcon } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';

export type MenuItem =
  | {
      label: string;
      /** An `ICONS.x` meaning from @uwusuite/design. */
      icon?: LucideIcon;
      danger?: boolean;
      disabled?: boolean;
      onSelect: () => void;
    }
  | 'separator';

type Props = {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
  label: string;
};

/**
 * A small menu at the pointer, for right-clicks. Arrow keys move, Enter picks,
 * Escape or a click anywhere else closes. It moves itself inside the window
 * when it would stick out. It looks like @uwusuite/design's `Menu`, which
 * opens below its button; this one opens wherever the pointer is.
 */
export function ContextMenu({ x, y, items, onClose, label }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ x, y });

  useLayoutEffect(() => {
    const menu = ref.current;
    if (!menu) return;
    const rect = menu.getBoundingClientRect();
    setPosition({
      x: Math.max(8, Math.min(x, window.innerWidth - rect.width - 8)),
      y: Math.max(8, Math.min(y, window.innerHeight - rect.height - 8)),
    });
    menu.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  }, [x, y]);

  useEffect(() => {
    const close = (event: Event) => {
      if (ref.current && event.target instanceof Node && ref.current.contains(event.target)) return;
      onClose();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        return;
      }
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      event.preventDefault();
      const buttons = [
        ...(ref.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []),
      ];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const step = event.key === 'ArrowDown' ? 1 : -1;
      buttons[(index + step + buttons.length) % buttons.length]?.focus();
    };
    window.addEventListener('pointerdown', close, true);
    window.addEventListener('keydown', key, true);
    window.addEventListener('blur', onClose);
    window.addEventListener('resize', onClose);
    return () => {
      window.removeEventListener('pointerdown', close, true);
      window.removeEventListener('keydown', key, true);
      window.removeEventListener('blur', onClose);
      window.removeEventListener('resize', onClose);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="context-menu fixed z-[var(--uwu-z-menu)] flex w-max max-w-[min(360px,calc(100vw-16px))] min-w-[210px] animate-pop flex-col rounded-2xl border border-line bg-surface p-1.5 text-ink shadow-float"
      role="menu"
      aria-label={label}
      style={{ left: position.x, top: position.y }}
      onContextMenu={(event) => event.preventDefault()}
    >
      {items.map((item, index) =>
        item === 'separator' ? (
          <hr key={`separator-${index}`} className="mx-2 my-1 border-hairline" />
        ) : (
          <button
            key={item.label}
            type="button"
            role="menuitem"
            disabled={item.disabled}
            data-danger={item.danger || undefined}
            className={cx(
              'flex items-center gap-2.5 rounded-xl border-0 bg-transparent px-3 py-2 text-left text-meta font-medium break-words hover:bg-pink-tint/60 focus:bg-pink-tint/60 focus:outline-none contrast-high:focus:outline-2 contrast-high:focus:-outline-offset-2 contrast-high:focus:outline-ink disabled:opacity-50',
              item.danger ? 'text-danger-ink' : 'text-ink',
            )}
            onClick={() => {
              onClose();
              item.onSelect();
            }}
          >
            {item.icon ? (
              <Icon icon={item.icon} className={item.danger ? undefined : 'text-muted'} />
            ) : (
              <span className="w-4 shrink-0" />
            )}
            {item.label}
          </button>
        ),
      )}
    </div>
  );
}
