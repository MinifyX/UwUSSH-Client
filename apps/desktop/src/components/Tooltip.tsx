import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

/** Room kept between the tooltip and the window's edge. */
const EDGE = 8;

/**
 * A tooltip for more than a `title` can hold: shown on hover and on focus,
 * drawn into <body> so a scrolling sidebar can't clip it, and kept inside the
 * window. While it shows, a `title` on the element around it is held back,
 * so the two never show at once.
 *
 * @uwusuite/design's `Tooltip` only takes a string; this one holds a few
 * lines with a heading (the password-login warning), and looks like the
 * package's: an ink bubble with the float shadow.
 */
export function Tooltip({
  content,
  children,
  className,
}: {
  content: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  const anchorRef = useRef<HTMLSpanElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);
  const heldTitle = useRef<{ element: HTMLElement; title: string } | null>(null);
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  const [place, setPlace] = useState<{ left: number; top: number } | null>(null);
  const id = useId();

  const show = () => {
    const element = anchorRef.current;
    if (!element) return;
    const titled = element.parentElement?.closest<HTMLElement>('[title]');
    if (titled && !heldTitle.current) {
      heldTitle.current = { element: titled, title: titled.title };
      titled.removeAttribute('title');
    }
    setAnchor(element.getBoundingClientRect());
  };

  const hide = () => {
    const held = heldTitle.current;
    if (held) held.element.title = held.title;
    heldTitle.current = null;
    setAnchor(null);
    setPlace(null);
  };

  // A row that goes away while hovered gets its title back all the same.
  useEffect(
    () => () => {
      const held = heldTitle.current;
      if (held) held.element.title = held.title;
    },
    [],
  );

  // Below the anchor, or above it when there is no room; never off the side.
  useLayoutEffect(() => {
    const tip = tipRef.current;
    if (!anchor || !tip) return;
    const { width, height } = tip.getBoundingClientRect();
    const left = Math.max(EDGE, Math.min(anchor.left, window.innerWidth - width - EDGE));
    const below = anchor.bottom + 6;
    const top = below + height + EDGE > window.innerHeight ? anchor.top - height - 6 : below;
    setPlace({ left, top: Math.max(EDGE, top) });
  }, [anchor]);

  return (
    <span
      ref={anchorRef}
      className={className ? `tooltip-anchor ${className}` : 'tooltip-anchor'}
      onMouseEnter={show}
      onMouseLeave={hide}
      onFocus={show}
      onBlur={hide}
      aria-describedby={anchor ? id : undefined}
    >
      {children}
      {anchor &&
        createPortal(
          <div
            ref={tipRef}
            id={id}
            role="tooltip"
            className="pointer-events-none fixed z-[var(--uwu-z-tooltip)] w-max max-w-[min(300px,calc(100vw-16px))] animate-fade rounded-lg bg-ink px-3 py-2 text-caption leading-relaxed font-normal text-canvas shadow-float"
            style={
              place
                ? { left: place.left, top: place.top }
                : { left: anchor.left, top: anchor.bottom + 6, visibility: 'hidden' }
            }
          >
            {content}
          </div>,
          document.body,
        )}
    </span>
  );
}
