/**
 * Nyu, the terminal cat (the `terminal` shell of @uwusuite/design).
 *
 * The palette, the sticker edge, the ears and the face come from the package;
 * what stays here is `NyuFigure`, the cat as a group that scenes place, scale
 * and tilt, and the scene paw. The colours are fixed artwork, not theme
 * tokens: a sticker looks like itself in dark mode too, and the white die-cut
 * edge is what keeps the outlines readable on a dark ground.
 *
 * Shared with the installer (apps/setup, alias `@nyu`), so nothing in here may
 * depend on the app's stylesheet beyond the package's `nyu.css`.
 */

import { NYU, Nyu as SuiteNyu, NyuEars, NyuFace, Sticker, type NyuMood } from '@uwusuite/design';
import type { ReactNode } from 'react';

export { NYU, Sticker, type NyuMood };

/** A paw in Nyu's own coordinates (the body spans 28–228 × 74–220), sized for the scenes. */
export function Paw({ x, y, className }: { x: number; y: number; className?: string }) {
  return (
    <g className={className}>
      <ellipse cx={x} cy={y} rx="19" ry="16" fill={NYU.body} stroke={NYU.ink} strokeWidth={9} />
      <path
        d={`M${x - 5} ${y + 3} v6 M${x + 5} ${y + 3} v6`}
        fill="none"
        stroke={NYU.ink}
        strokeWidth={5}
      />
    </g>
  );
}

type FigureProps = {
  mood?: NyuMood;
  /** Centre of the body in the parent's coordinates. */
  x?: number;
  y?: number;
  /** 1 is the size of the app symbol: the body is 200 wide. */
  scale?: number;
  tilt?: number;
  /** Extra parts in Nyu's own coordinates. */
  behind?: ReactNode;
  front?: ReactNode;
  /** Replaces the mood's eyes, e.g. pupils that follow something. The mouth stays the mood's. */
  eyes?: ReactNode;
  /** The white die-cut edge, in Nyu's own coordinates. */
  edge?: number;
};

/** Nyu as a group, for scenes: placed, scaled and tilted in the parent's coordinates. */
export function NyuFigure({
  mood = 'uwu',
  x = 128,
  y = 147,
  scale = 1,
  tilt = 0,
  behind,
  front,
  eyes,
  edge = 20,
}: FigureProps) {
  return (
    <g
      transform={`translate(${x} ${y}) rotate(${tilt}) scale(${scale}) translate(-128 -147)`}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <Sticker edge={edge}>
        {behind}
        <NyuEars />
        <rect
          x={28}
          y={74}
          width={200}
          height={146}
          rx={24}
          fill={NYU.body}
          stroke={NYU.ink}
          strokeWidth={9}
        />
        <g className="no-edge" fill={NYU.flap}>
          <circle cx={52} cy={98} r={5.5} />
          <circle cx={70} cy={98} r={5.5} />
          <circle cx={88} cy={98} r={5.5} />
        </g>
        <rect
          x={44}
          y={116}
          width={168}
          height={88}
          rx={16}
          fill={NYU.flap}
          stroke={NYU.ink}
          strokeWidth={7}
        />
        <NyuFace mood={mood} eyes={eyes} />
        <rect
          className="no-edge nyu-cursor"
          x={180}
          y={184}
          width={12}
          height={6}
          rx={1.5}
          fill={NYU.ink}
        />
        {front}
      </Sticker>
    </g>
  );
}

type NyuProps = {
  size?: number;
  mood?: NyuMood;
  /** Blinking is on by default and stops on its own when motion is reduced. */
  blink?: boolean;
  title?: string;
  className?: string;
};

/**
 * The symbol on its own: the host list, About, the update hint. The package's
 * terminal Nyu is cropped to the cat (230 of 256 units high); `size` stays the
 * old 256-unit box, so the cat keeps the size it had.
 */
export function Nyu({ size = 96, mood = 'uwu', blink = true, title = 'Nyu', className }: NyuProps) {
  return (
    <SuiteNyu
      shell="terminal"
      mood={mood}
      size={Math.round((size * 230) / 256)}
      blink={blink}
      title={title}
      className={className}
    />
  );
}
