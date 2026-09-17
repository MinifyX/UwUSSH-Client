import type { ReactNode } from 'react';
import { NYU, NyuFigure, Paw, Sticker } from './Nyu';

// Every scene is drawn on a 320 × 220 canvas, the same as UwUMail's. Nyu sits
// at about 0.6 scale, so props use a 6 px outline and an 18 px edge to match.

const S = { stroke: NYU.outline, strokeWidth: 6 } as const;
const EDGE = 18;
/** Nyu's own edge at scene scale: 30 × 0.6 ≈ the props' 18 px. */
const NYU_EDGE = 30;

export function Shadow({ cx = 160, rx = 104 }: { cx?: number; rx?: number }) {
  return (
    <ellipse
      className="no-edge"
      cx={cx}
      cy="204"
      rx={rx}
      ry="8"
      fill={NYU.outline}
      opacity="0.08"
    />
  );
}

export function Star({ x, y, r = 12 }: { x: number; y: number; r?: number }) {
  const k = r * 0.2;
  return (
    <path
      d={`M${x} ${y - r} Q${x + k} ${y - k} ${x + r} ${y} Q${x + k} ${y + k} ${x} ${y + r} Q${x - k} ${y + k} ${x - r} ${y} Q${x - k} ${y - k} ${x} ${y - r}Z`}
      fill={NYU.star}
      stroke={NYU.outline}
      strokeWidth={r > 10 ? 4 : 3}
    />
  );
}

export function Heart({
  x,
  y,
  size = 1,
  fill = NYU.body,
}: {
  x: number;
  y: number;
  size?: number;
  fill?: string;
}) {
  return (
    <path
      transform={`translate(${x} ${y}) scale(${size})`}
      d="M0 13 C-15 3 -18 -4 -17 -8 C-16 -15 -7 -16 -3 -11 L0 -8 L3 -11 C7 -16 16 -15 17 -8 C18 -4 15 3 0 13Z"
      fill={fill}
      stroke={NYU.outline}
      strokeWidth={4 / size}
    />
  );
}

/** The key Nyu guards, as on the app icon. */
export function Key({
  x,
  y,
  rotate = 0,
  size = 1,
}: {
  x: number;
  y: number;
  rotate?: number;
  size?: number;
}) {
  return (
    <g transform={`translate(${x} ${y}) rotate(${rotate}) scale(${size})`}>
      <g fill="none" stroke={NYU.outline} strokeWidth={6}>
        <path d="M0 4 V40 M0 24 H13 M0 35 H10" />
      </g>
      <circle cx="0" cy="-9" r="13" fill={NYU.lilac} {...S} />
      <circle className="no-edge" cx="0" cy="-9" r="4.5" fill={NYU.tile} />
    </g>
  );
}

/** A little terminal window with a prompt, the kind Nyu hands around. */
export function Prompt({
  x,
  y,
  rotate = 0,
  cursor = false,
}: {
  x: number;
  y: number;
  rotate?: number;
  cursor?: boolean;
}) {
  return (
    <g transform={`translate(${x} ${y}) rotate(${rotate})`}>
      <rect x="-26" y="-18" width="52" height="36" rx="7" fill={NYU.paper} {...S} strokeWidth={5} />
      <path d="M-26 -8 H26" stroke={NYU.outline} strokeWidth={4} />
      <g className="no-edge" fill={NYU.body}>
        <circle cx="-19" cy="-13" r="2" />
        <circle cx="-13" cy="-13" r="2" />
      </g>
      <path d="M-16 0 L-9 5 L-16 10" fill="none" stroke={NYU.body} strokeWidth={4} />
      <path
        className={cursor ? 'no-edge nyu-cursor' : 'no-edge'}
        d="M-4 10 H8"
        stroke={NYU.outline}
        strokeWidth={4}
      />
    </g>
  );
}

/** First start: Nyu says hello. */
function Welcome() {
  return (
    <>
      <Shadow />
      <path
        d="M246 44 q12 9 10 25 M262 32 q16 13 14 35"
        fill="none"
        stroke={NYU.outline}
        strokeWidth={4}
        opacity="0.4"
      />
      <NyuFigure
        mood="happy"
        x={150}
        y={134}
        scale={0.62}
        tilt={-6}
        edge={NYU_EDGE}
        front={<Paw x={238} y={104} className="nyu-wave" />}
      />
      <Sticker edge={12}>
        <Heart x={50} y={62} size={0.95} />
        <Star x={286} y={150} r={11} />
        <Star x={36} y={150} r={8} />
      </Sticker>
      <Sticker edge={EDGE}>
        <Key x={250} y={176} rotate={62} size={0.9} />
      </Sticker>
    </>
  );
}

const CONFETTI: [x: number, y: number, rotate: number, fill: string][] = [
  [42, 40, -20, NYU.body],
  [78, 16, 30, NYU.star],
  [118, 30, 70, NYU.mint],
  [210, 22, -40, NYU.lilac],
  [250, 44, 15, NYU.body],
  [284, 20, 60, NYU.sky],
  [30, 108, 45, NYU.sky],
  [292, 104, -30, NYU.star],
  [48, 170, 20, NYU.lilac],
  [276, 172, -60, NYU.mint],
];

/** Setup done: Nyu cheers with both paws up. */
function Done() {
  return (
    <>
      <Shadow />
      <Sticker edge={10}>
        {CONFETTI.map(([x, y, rotate, fill]) => (
          <rect
            key={`${x}-${y}`}
            x={x - 7}
            y={y - 4}
            width="14"
            height="8"
            rx="2"
            transform={`rotate(${rotate} ${x} ${y})`}
            fill={fill}
            stroke={NYU.outline}
            strokeWidth={3}
          />
        ))}
      </Sticker>
      <NyuFigure
        mood="cheer"
        x={160}
        y={138}
        scale={0.62}
        edge={NYU_EDGE}
        front={
          <>
            <Paw x={24} y={112} />
            <Paw x={232} y={112} />
          </>
        }
      />
      <Sticker edge={12}>
        <Star x={160} y={34} r={11} />
      </Sticker>
    </>
  );
}

/** Something failed: Nyu got tangled in a network cable. */
function LoadError() {
  const cable = 'M34 190 C66 160 90 208 124 176 S206 118 210 160 S140 180 182 198 S246 194 256 176';
  return (
    <>
      <Shadow cx={150} />
      <NyuFigure mood="sad" x={146} y={122} scale={0.6} tilt={6} edge={NYU_EDGE} />
      <Sticker edge={16}>
        <path d={cable} fill="none" stroke={NYU.outline} strokeWidth={12} />
      </Sticker>
      <path d={cable} fill="none" stroke={NYU.violet} strokeWidth={5} />
      <Sticker edge={EDGE}>
        <g transform="rotate(-28 270 168)">
          <path d="M284 162 h12 M284 174 h12" stroke={NYU.outline} strokeWidth={5} />
          <rect x="254" y="156" width="32" height="24" rx="6" fill={NYU.lilac} {...S} />
        </g>
      </Sticker>
    </>
  );
}

/** Something needs a decision first: Nyu holds up a page with a question mark. */
function Puzzled() {
  return (
    <>
      <Shadow cx={150} />
      <NyuFigure mood="puzzled" x={112} y={134} scale={0.6} tilt={-8} edge={NYU_EDGE} />
      <Sticker edge={EDGE}>
        <g transform="rotate(8 226 118)">
          <path d="M190 58 H240 L262 80 V176 H190Z" fill={NYU.paper} {...S} />
          <path d="M240 58 V80 H262" fill={NYU.screen} {...S} />
          <path
            d="M212 106 q0 -15 15 -15 q15 0 15 13 q0 10 -13 14 v8"
            fill="none"
            stroke={NYU.body}
            strokeWidth={9}
          />
          <circle cx="229" cy="145" r="5.5" fill={NYU.body} />
        </g>
        <ellipse cx="186" cy="138" rx="12" ry="10" fill={NYU.body} {...S} />
      </Sticker>
    </>
  );
}

/** No tab open: Nyu points at the host list, a prompt waiting next to her. */
function Pick() {
  return (
    <>
      <Shadow cx={170} />
      <Sticker edge={EDGE}>
        <Prompt x={264} y={70} rotate={10} cursor />
      </Sticker>
      <NyuFigure
        mood="happy"
        x={168}
        y={134}
        scale={0.6}
        tilt={4}
        edge={NYU_EDGE}
        front={<Paw x={16} y={130} className="nyu-wave" />}
      />
      <path d="M34 118 h-18 M40 132 h-14" stroke={NYU.outline} strokeWidth={4} opacity="0.35" />
      <Sticker edge={12}>
        <Star x={290} y={172} r={9} />
        <Star x={46} y={52} r={11} />
      </Sticker>
    </>
  );
}

/** Saying goodbye: Nyu waves with a little tear. */
function Goodbye() {
  return (
    <>
      <Shadow />
      <NyuFigure
        mood="sad"
        x={160}
        y={134}
        scale={0.62}
        tilt={4}
        edge={NYU_EDGE}
        front={<Paw x={240} y={104} className="nyu-wave" />}
      />
      <Sticker edge={12}>
        <Heart x={58} y={58} size={0.85} fill={NYU.lilac} />
        <Star x={280} y={40} r={9} />
      </Sticker>
    </>
  );
}

const SCENES = {
  welcome: Welcome,
  done: Done,
  loadError: LoadError,
  puzzled: Puzzled,
  pick: Pick,
  goodbye: Goodbye,
} satisfies Record<string, () => ReactNode>;

export type SceneName = keyof typeof SCENES;

/** A small illustration of Nyu for empty and error states. Decorative only. */
export function NyuScene({ name, className }: { name: SceneName; className?: string }) {
  const Scene = SCENES[name];
  return (
    <svg
      viewBox="-10 -10 340 230"
      className={className ? `nyu-host nyu-blink ${className}` : 'nyu-host nyu-blink'}
      style={{ overflow: 'visible' }}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <Scene />
    </svg>
  );
}
