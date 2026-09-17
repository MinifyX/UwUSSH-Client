/**
 * Nyu's laser pad: the play mat where moving the mouse collects randomness for
 * a new key.
 *
 * PuTTYgen asks you to wiggle the mouse over a grey box. Here the pointer turns
 * into a laser dot and Nyu tries to catch it. Everything that moves per frame
 * runs in one requestAnimationFrame loop that writes transforms straight to the
 * DOM; React only re-renders when her pose changes, a few times a second at
 * most. The loop stops once the pointer is gone and she has settled.
 *
 * The pad does no cryptography. Every pointer sample goes to `onEntropy` as a
 * few raw bytes, and the caller hashes them into its seed.
 */

import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react';
import { NYU, NyuFigure, Paw, Sticker, type NyuMood } from '../nyu/Nyu';
import { Heart, Star } from '../nyu/scenes';
import './nyu-laser.css';

export type NyuLaserPadProps = {
  /** 0..1 — how much movement was collected; the pad draws it as a trail of paw prints. */
  progress: number;
  /**
   * Called for every pointer movement sample with a few raw bytes (x, y, timing,
   * movementX/Y, pressure, pointer type) for the caller to mix into the key seed.
   */
  onEntropy: (sample: Uint8Array) => void;
  /** When false (e.g. already collected, or generating), the pad shows a calm/done state and stops collecting. */
  active: boolean;
  /** Optional: shown under the pad, e.g. "Bewege die Maus über das Feld". */
  hint?: string;
  className?: string;
  /** Accessible name of the pad. */
  label?: string;
  /** Accessible name of the progress bar. */
  progressLabel?: string;
};

// The mat is drawn on a 420 × 240 canvas and the pad keeps that aspect ratio,
// so pointer positions map onto it linearly.
const W = 420;
const H = 240;
/** Nyu's scale on the mat: her body is 100 units wide, her ears reach 92 up. */
const S = 0.5;
const NYU_EDGE = 30;
/** Where she sits when nothing is going on, as the point between her feet. */
const HOME = { x: 210, y: 204 };
/** Her feet stay in here. The tail sticks out further right than her left side does. */
const FEET = { minX: 60, maxX: 334, minY: 120, maxY: 206 };
/** The middle of her screen, above her feet. */
const FACE_Y = (160 - 220) * S;
/** How far her paw reaches from her face. */
const REACH = 62;
const CATCH_RADIUS = 22;
/** Pad units per second. Faster than this and she gives chase. */
const CHASE_SPEED = 820;
const SPARKLE_SPEED = 1100;
const TRAIL = 6;
const SPARKS = 4;
const PRINTS = 12;

type Phase = 'calm' | 'done' | 'idle' | 'watch' | 'crouch' | 'pounce' | 'caught';
/** What React draws. The idle phase has three looks, switched by timers. */
type Pose = Phase | 'wonder' | 'sleep';

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
const round = (value: number) => Math.round(value * 100) / 100;

/**
 * Settings → Animations. The app puts its decision on <html> (see
 * `applyAppearance`), the system's preference included, so "An" can overrule
 * a system that asks for less motion.
 */
function motionReduced(): boolean {
  return document.documentElement.dataset.motion === 'reduced';
}

const POINTER_KIND: Record<string, number> = { mouse: 1, pen: 2, touch: 3 };
const KEYBOARD_KIND = 4;

/**
 * One entropy sample, 16 bytes, big-endian:
 *
 *    0–1   x in 1/16 CSS px (low 16 bits)     2–3   y, the same
 *    4–7   event time in µs (low 32 bits)     8–11  time we handled it, in µs
 *   12     movementX (clamped to a byte)     13     movementY
 *   14     pressure × 255 (keys: which key)  15     pointer kind | buttons << 3
 *
 * Most of it is predictable; the low bits of the timings and positions are not.
 * The caller hashes everything, so nothing here needs to be uniform.
 */
function sample(
  x: number,
  y: number,
  time: number,
  moveX: number,
  moveY: number,
  level: number,
  kind: number,
  buttons: number,
): Uint8Array {
  const bytes = new Uint8Array(16);
  const view = new DataView(bytes.buffer);
  view.setUint16(0, Math.round(x * 16) & 0xffff);
  view.setUint16(2, Math.round(y * 16) & 0xffff);
  view.setUint32(4, Math.floor(time * 1000) >>> 0);
  view.setUint32(8, Math.floor(performance.now() * 1000) >>> 0);
  view.setInt8(12, clamp(Math.round(moveX) || 0, -128, 127));
  view.setInt8(13, clamp(Math.round(moveY) || 0, -128, 127));
  bytes[14] = clamp(Math.round(level) || 0, 0, 255);
  bytes[15] = (kind & 0x7) | ((buttons & 0x1f) << 3);
  return bytes;
}

type Sim = {
  phase: Phase;
  phaseAt: number;
  pose: Pose;
  pointerInside: boolean;
  keyboard: boolean;
  /** Laser position and how fast it moves, in pad units. */
  laserX: number;
  laserY: number;
  prevX: number;
  prevY: number;
  speed: number;
  stillX: number;
  stillY: number;
  stillSince: number;
  /** Earlier laser positions, newest first, as x, y pairs. */
  trail: number[];
  trailLevel: number;
  lastSpark: number;
  nextSpark: number;
  /** Nyu: her feet on the mat, a spring pulling them to a target. */
  x: number;
  y: number;
  vx: number;
  vy: number;
  tx: number;
  ty: number;
  stiffness: number;
  damping: number;
  hopping: boolean;
  hopStart: number;
  hopDuration: number;
  hopHeight: number;
  lift: number;
  squash: number;
  squashV: number;
  tilt: number;
  wiggle: number;
  lookX: number;
  lookY: number;
  dilate: number;
  /** Distance and direction from her face to the laser. */
  dist: number;
  ux: number;
  uy: number;
  /** Where she pinned the dot. */
  caughtX: number;
  caughtY: number;
  cooldownUntil: number;
  frame: number;
  last: number;
};

function createSim(): Sim {
  return {
    phase: 'idle',
    phaseAt: 0,
    pose: 'idle',
    pointerInside: false,
    keyboard: false,
    laserX: W * 0.72,
    laserY: H * 0.4,
    prevX: W * 0.72,
    prevY: H * 0.4,
    speed: 0,
    stillX: 0,
    stillY: 0,
    stillSince: 0,
    trail: Array.from({ length: TRAIL * 2 }, (_, i) => (i % 2 ? H * 0.4 : W * 0.72)),
    trailLevel: 0,
    lastSpark: 0,
    nextSpark: 0,
    x: HOME.x,
    y: HOME.y,
    vx: 0,
    vy: 0,
    tx: HOME.x,
    ty: HOME.y,
    stiffness: 14,
    damping: 7.5,
    hopping: false,
    hopStart: 0,
    hopDuration: 1,
    hopHeight: 0,
    lift: 0,
    squash: 1,
    squashV: 0,
    tilt: 0,
    wiggle: 0,
    lookX: 0,
    lookY: 0,
    dilate: 1,
    dist: 100,
    ux: 1,
    uy: 0,
    caughtX: 0,
    caughtY: 0,
    cooldownUntil: 0,
    frame: 0,
    last: 0,
  };
}

type Parts = {
  eyes: { el: SVGGElement; cx: number }[];
  reach: SVGGElement[];
  catchL: SVGGElement[];
  catchR: SVGGElement[];
};

const LOOKS: Record<Pose, { mood: NyuMood; track: boolean }> = {
  calm: { mood: 'uwu', track: false },
  done: { mood: 'sparkle', track: false },
  idle: { mood: 'happy', track: true },
  wonder: { mood: 'puzzled', track: false },
  sleep: { mood: 'sleepy', track: false },
  watch: { mood: 'happy', track: true },
  crouch: { mood: 'happy', track: true },
  pounce: { mood: 'cheer', track: true },
  caught: { mood: 'cheer', track: false },
};

/** The happy eyes, split so the loop can move and widen each pupil. */
const TRACKING_EYES = (
  <g className="nyu-laser-glance">
    {[102, 154].map((cx) => (
      <g key={cx} data-part="eye" data-cx={cx} transform={`translate(${cx} 148)`}>
        <ellipse rx="8" ry="10" fill={NYU.outline} />
        <circle cx="3" cy="-4" r="3" fill={NYU.paper} />
      </g>
    ))}
  </g>
);

/** Nyu has no tail in the symbol; on the mat she gets one to swish. */
const TAIL_PATH = 'M212 202 C252 214 272 200 268 172 C265 152 272 136 292 136';
const TAIL = (
  <g className="nyu-laser-tail">
    <path
      className="nyu-laser-tail-line"
      d={TAIL_PATH}
      fill="none"
      stroke={NYU.outline}
      strokeWidth={21}
    />
    <path className="no-edge" d={TAIL_PATH} fill="none" stroke={NYU.body} strokeWidth={12} />
  </g>
);

function frontFor(pose: Pose): ReactNode {
  switch (pose) {
    case 'crouch':
      return (
        <>
          <Paw x={98} y={214} />
          <Paw x={158} y={214} />
        </>
      );
    case 'pounce':
      return (
        <g data-part="reach" transform="translate(240 160)">
          <Paw x={0} y={0} />
        </g>
      );
    case 'caught':
      return (
        <>
          <g data-part="catch-l" transform="translate(200 200)">
            <Paw x={0} y={0} />
          </g>
          <g data-part="catch-r" transform="translate(240 200)">
            <Paw x={0} y={0} />
          </g>
        </>
      );
    case 'done':
      return (
        <>
          <Paw x={22} y={118} />
          <Paw x={234} y={118} className="nyu-wave" />
        </>
      );
    default:
      return null;
  }
}

/** Hearts and stars that pop off the dot when she catches it. Drawn around 0, 0. */
const BURST: { item: ReactNode; angle: number; distance: number }[] = [
  {
    item: (
      <Sticker edge={16}>
        <Heart x={0} y={0} size={0.7} />
      </Sticker>
    ),
    angle: -100,
    distance: 50,
  },
  {
    item: (
      <Sticker edge={10}>
        <Star x={0} y={0} r={9} />
      </Sticker>
    ),
    angle: -150,
    distance: 44,
  },
  {
    item: (
      <Sticker edge={18}>
        <Heart x={0} y={0} size={0.55} fill={NYU.lilac} />
      </Sticker>
    ),
    angle: -40,
    distance: 46,
  },
  {
    item: (
      <Sticker edge={9}>
        <Star x={0} y={0} r={7} />
      </Sticker>
    ),
    angle: -65,
    distance: 62,
  },
  {
    item: (
      <Sticker edge={18}>
        <Heart x={0} y={0} size={0.5} />
      </Sticker>
    ),
    angle: -130,
    distance: 64,
  },
];

/** Where the confetti ends up, around Nyu's head at home: dx, dy, rotation, colour. */
const CONFETTI: [dx: number, dy: number, rotate: number, fill: string][] = [
  [-118, -62, -20, NYU.body],
  [-80, -100, 30, NYU.star],
  [-34, -118, 70, NYU.mint],
  [40, -116, -40, NYU.lilac],
  [90, -94, 15, NYU.body],
  [126, -56, 60, NYU.sky],
  [-146, -6, 45, NYU.sky],
  [150, -2, -30, NYU.star],
  [-104, 36, 20, NYU.lilac],
  [112, 38, -60, NYU.mint],
];
const HEAD = { x: HOME.x, y: HOME.y - 60 };

function PawPrint() {
  return (
    <>
      <path d="M-6.5 3.5 C-6.5 -1.5 -3 -2.5 0 -2.5 C3 -2.5 6.5 -1.5 6.5 3.5 C6.5 7 3 7.5 0 6.5 C-3 7.5 -6.5 7 -6.5 3.5Z" />
      <ellipse cx="-7.2" cy="-5.2" rx="2.3" ry="3" transform="rotate(-25 -7.2 -5.2)" />
      <ellipse cx="-2.5" cy="-8.6" rx="2.3" ry="3" />
      <ellipse cx="2.5" cy="-8.6" rx="2.3" ry="3" />
      <ellipse cx="7.2" cy="-5.2" rx="2.3" ry="3" transform="rotate(25 7.2 -5.2)" />
    </>
  );
}

/** Progress as a little walk of paw prints along the bottom edge of the mat. */
function PawPrints({ progress }: { progress: number }) {
  const filled = clamp(progress, 0, 1) * PRINTS;
  return (
    <g>
      {Array.from({ length: PRINTS }, (_, i) => {
        const x = 34 + (i * (W - 68)) / (PRINTS - 1);
        const up = i % 2 === 1;
        const amount = clamp(filled - i, 0, 1);
        return (
          <g
            key={i}
            transform={`translate(${round(x)} ${up ? 221 : 229}) rotate(${up ? 80 : 100})`}
          >
            <g className="nyu-laser-print-empty">
              <PawPrint />
            </g>
            {amount > 0 && (
              <g
                className="nyu-laser-print"
                data-full={amount >= 1}
                opacity={amount >= 1 ? 1 : round(0.25 + amount * 0.5)}
              >
                <Sticker edge={4.5}>
                  <g fill={NYU.body} stroke={NYU.outline} strokeWidth={1.6}>
                    <PawPrint />
                  </g>
                </Sticker>
              </g>
            )}
          </g>
        );
      })}
    </g>
  );
}

export function NyuLaserPad({
  progress,
  onEntropy,
  active,
  hint,
  className,
  label = 'Zufallsfeld',
  progressLabel = 'Gesammelter Zufall',
}: NyuLaserPadProps): JSX.Element {
  const hintId = useId();
  const padRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<SVGSVGElement>(null);
  const catRef = useRef<SVGGElement>(null);
  const shadowRef = useRef<SVGEllipseElement>(null);
  const liftRef = useRef<SVGGElement>(null);
  const bodyRef = useRef<SVGGElement>(null);
  const dotRef = useRef<HTMLDivElement>(null);
  const trailRefs = useRef<(HTMLDivElement | null)[]>([]);
  const sparkRefs = useRef<(HTMLDivElement | null)[]>([]);
  const burstRefs = useRef<(SVGGElement | null)[]>([]);
  const confettiRefs = useRef<(SVGGElement | null)[]>([]);
  const parts = useRef<Parts>({ eyes: [], reach: [], catchL: [], catchR: [] });

  const simRef = useRef<Sim | null>(null);
  simRef.current ??= createSim();
  const [pose, setPose] = useState<Pose>(() => (active ? 'idle' : progress >= 1 ? 'done' : 'calm'));

  // The loop and the listeners live for the whole mount and read props through refs.
  const activeRef = useRef(active);
  const progressRef = useRef(progress);
  const onEntropyRef = useRef(onEntropy);
  const controls = useRef<{ syncActive: () => void; redraw: () => void } | null>(null);

  useLayoutEffect(() => {
    activeRef.current = active;
    progressRef.current = progress;
    onEntropyRef.current = onEntropy;
  });

  useEffect(() => {
    const pad = padRef.current;
    const sim = simRef.current;
    if (!pad || !sim) return;

    let rect: DOMRect | null = null;
    const bounds = () => (rect ??= pad.getBoundingClientRect());
    const invalidate = () => {
      rect = null;
    };

    const timers: number[] = [];
    const clearTimers = () => {
      for (const timer of timers) window.clearTimeout(timer);
      timers.length = 0;
    };

    const wake = () => {
      if (sim.frame) return;
      sim.last = performance.now();
      sim.frame = requestAnimationFrame(tick);
    };

    const applyPose = (next: Pose) => {
      if (sim.pose === next) return;
      sim.pose = next;
      setPose(next);
      wake();
    };

    const setPhase = (phase: Phase, now: number) => {
      sim.phase = phase;
      sim.phaseAt = now;
      if (phase === 'idle' || phase === 'calm' || phase === 'done') {
        // A gentle walk home.
        sim.tx = HOME.x;
        sim.ty = HOME.y;
        sim.stiffness = 14;
        sim.damping = 7.5;
      }
      if (phase !== 'idle') applyPose(phase);
    };

    const showInside = () => {
      pad.dataset.inside = String(sim.pointerInside || sim.keyboard);
    };

    const goIdle = (wondering: boolean) => {
      clearTimers();
      showInside();
      if (!activeRef.current) return;
      setPhase('idle', performance.now());
      applyPose(wondering ? 'wonder' : 'idle');
      if (wondering) {
        timers.push(window.setTimeout(() => sim.phase === 'idle' && applyPose('idle'), 1800));
      }
      timers.push(window.setTimeout(() => sim.phase === 'idle' && applyPose('sleep'), 9000));
      wake();
    };

    const goInside = () => {
      clearTimers();
      showInside();
      if (!activeRef.current) return;
      const now = performance.now();
      if (sim.pose === 'sleep' && !motionReduced()) {
        // Startled awake: a little hop on the spot.
        sim.hopping = true;
        sim.hopStart = now;
        sim.hopDuration = 240;
        sim.hopHeight = 10;
      }
      sim.stillX = sim.laserX;
      sim.stillY = sim.laserY;
      sim.stillSince = now;
      sim.cooldownUntil = now + 300;
      setPhase('watch', now);
      wake();
    };

    const placeLaser = (x: number, y: number, now: number, jump: boolean) => {
      sim.laserX = clamp(x, 0, W);
      sim.laserY = clamp(y, 0, H);
      if (jump) {
        // Coming in from outside: no streak from where the dot left.
        sim.prevX = sim.laserX;
        sim.prevY = sim.laserY;
        for (let i = 0; i < sim.trail.length; i += 2) {
          sim.trail[i] = sim.laserX;
          sim.trail[i + 1] = sim.laserY;
        }
      }
      if (Math.hypot(sim.laserX - sim.stillX, sim.laserY - sim.stillY) > 6) {
        sim.stillX = sim.laserX;
        sim.stillY = sim.laserY;
        sim.stillSince = now;
      }
    };

    const toPad = (clientX: number, clientY: number): [number, number] => {
      const r = bounds();
      return [((clientX - r.left) * W) / r.width, ((clientY - r.top) * H) / r.height];
    };

    const launch = (now: number) => {
      // Land where the paw meets the dot, but in hops, and on the mat.
      let tx = sim.laserX - sim.ux * REACH;
      let ty = sim.laserY - sim.uy * REACH - FACE_Y;
      const hx = tx - sim.x;
      const hy = ty - sim.y;
      const length = Math.hypot(hx, hy);
      if (length > 150) {
        tx = sim.x + (hx / length) * 150;
        ty = sim.y + (hy / length) * 150;
      }
      tx = clamp(tx, FEET.minX, FEET.maxX);
      ty = clamp(ty, FEET.minY, FEET.maxY);
      const hop = Math.hypot(tx - sim.x, ty - sim.y);
      sim.hopping = true;
      sim.hopStart = now;
      sim.hopDuration = clamp(260 + hop * 1.6, 280, 520);
      // High enough to look like a jump, low enough to keep her ears on the mat.
      sim.hopHeight = Math.max(6, Math.min(12 + hop * 0.22, Math.min(sim.y, ty) - 98));
      // A spring that arrives with the landing and overshoots a touch: she slides.
      const omega = 4.4 / (sim.hopDuration / 1000);
      sim.stiffness = omega * omega;
      sim.damping = 2 * 0.75 * omega;
      sim.tx = tx;
      sim.ty = ty;
      setPhase('pounce', now);
    };

    const burst = () => {
      if (motionReduced()) return;
      BURST.forEach(({ angle, distance }, i) => {
        const holder = burstRefs.current[i];
        const item = holder?.firstElementChild as SVGGElement | null | undefined;
        if (!holder || !item || typeof item.animate !== 'function') return;
        holder.setAttribute('transform', `translate(${round(sim.caughtX)} ${round(sim.caughtY)})`);
        const dx = Math.cos((angle * Math.PI) / 180) * distance;
        const dy = Math.sin((angle * Math.PI) / 180) * distance;
        item.animate(
          [
            { transform: 'translate(0px, 0px) scale(0.2)', opacity: 0 },
            {
              transform: `translate(${dx * 0.45}px, ${dy * 0.45}px) scale(1.15)`,
              opacity: 1,
              offset: 0.25,
            },
            {
              transform: `translate(${dx * 0.9}px, ${dy * 0.9}px) scale(1)`,
              opacity: 1,
              offset: 0.7,
            },
            { transform: `translate(${dx}px, ${dy}px) scale(0.8)`, opacity: 0 },
          ],
          {
            duration: 900,
            delay: i * 45,
            easing: 'cubic-bezier(0.2, 0.7, 0.3, 1)',
            fill: 'backwards',
          },
        );
      });
    };

    const catchIt = (now: number) => {
      sim.caughtX = sim.laserX;
      sim.caughtY = sim.laserY;
      setPhase('caught', now);
      burst();
    };

    const release = (now: number, escaped: boolean) => {
      if (escaped) {
        sim.cooldownUntil = now + 500;
      } else {
        // She lets go with a little hop back.
        sim.tx = clamp(sim.x - Math.sign(sim.ux || 1) * 22, FEET.minX, FEET.maxX);
        sim.ty = sim.y;
        sim.stiffness = 90;
        sim.damping = 16;
        sim.hopping = true;
        sim.hopStart = now;
        sim.hopDuration = 260;
        sim.hopHeight = 9;
        sim.cooldownUntil = now + 900;
      }
      setPhase('watch', now);
    };

    const land = (now: number) => {
      sim.squashV -= 1.2;
      if (sim.phase !== 'pounce') return;
      if (sim.dist < REACH + CATCH_RADIUS) {
        catchIt(now);
      } else {
        sim.cooldownUntil = now + (sim.speed > CHASE_SPEED ? 90 : 350);
        setPhase('watch', now);
      }
    };

    const think = (now: number) => {
      switch (sim.phase) {
        case 'idle':
        case 'calm':
        case 'done':
          setPhase('watch', now);
          break;
        case 'watch':
          if (now < sim.cooldownUntil) break;
          if (sim.speed > CHASE_SPEED && sim.dist > REACH) launch(now);
          else if (now - sim.stillSince > 600 && sim.dist < 200) setPhase('crouch', now);
          break;
        case 'crouch': {
          const t = now - sim.phaseAt;
          if (sim.dist > 240) setPhase('watch', now);
          else if (t > 800 || (t > 150 && sim.speed > CHASE_SPEED)) launch(now);
          break;
        }
        case 'pounce':
          if (sim.hopping && (now - sim.hopStart) / sim.hopDuration > 0.4) {
            if (sim.dist < REACH + CATCH_RADIUS) catchIt(now);
          }
          break;
        case 'caught':
          // It slips away if it moves out from under her paws; otherwise she lets go after a moment.
          if (Math.hypot(sim.laserX - sim.caughtX, sim.laserY - sim.caughtY) > 40) {
            release(now, true);
          } else if (now - sim.phaseAt > 1250) {
            release(now, false);
          }
          break;
      }
    };

    const step = (now: number, dt: number, inside: boolean, reduced: boolean) => {
      // How fast the dot moves, smoothed over roughly 80 ms.
      const moved = Math.hypot(sim.laserX - sim.prevX, sim.laserY - sim.prevY);
      sim.prevX = sim.laserX;
      sim.prevY = sim.laserY;
      sim.speed += (moved / dt - sim.speed) * (1 - Math.exp(-dt / 0.08));
      sim.trail.unshift(sim.laserX, sim.laserY);
      sim.trail.length = TRAIL * 2;

      const measure = () => {
        const dx = sim.laserX - sim.x;
        const dy = sim.laserY - (sim.y - sim.lift + FACE_Y);
        sim.dist = Math.hypot(dx, dy) || 1;
        sim.ux = dx / sim.dist;
        sim.uy = dy / sim.dist;
      };
      measure();

      if (reduced) {
        // Only her eyes follow the dot.
        if (inside && sim.phase !== 'watch') setPhase('watch', now);
        sim.tx = sim.x;
        sim.ty = sim.y;
        sim.vx = sim.vy = 0;
        sim.hopping = false;
        sim.lift = 0;
        sim.squash = 1;
        sim.squashV = 0;
        sim.tilt = 0;
        sim.wiggle = 0;
      } else {
        if (inside) think(now);

        let squashTarget = 1;
        if (sim.phase === 'crouch') squashTarget = 0.8;
        else if (sim.phase === 'pounce' && sim.hopping) {
          squashTarget = (now - sim.hopStart) / sim.hopDuration < 0.5 ? 1.12 : 0.97;
        }

        // Springs, in small steps so a stiff pounce stays stable at low frame rates.
        const steps = Math.ceil(dt * 120);
        const h = dt / steps;
        for (let i = 0; i < steps; i++) {
          sim.vx += (sim.stiffness * (sim.tx - sim.x) - sim.damping * sim.vx) * h;
          sim.vy += (sim.stiffness * (sim.ty - sim.y) - sim.damping * sim.vy) * h;
          sim.x += sim.vx * h;
          sim.y += sim.vy * h;
          sim.squashV += (240 * (squashTarget - sim.squash) - 16 * sim.squashV) * h;
          sim.squash += sim.squashV * h;
        }

        let lift = 0;
        if (sim.hopping) {
          const t = (now - sim.hopStart) / sim.hopDuration;
          if (t >= 1) {
            sim.hopping = false;
            sim.lift = 0;
            measure();
            land(now);
          } else {
            lift = Math.sin(Math.PI * t) * sim.hopHeight;
          }
        } else if (
          Math.hypot(sim.vx, sim.vy) > 12 &&
          (sim.phase === 'idle' || sim.phase === 'calm' || sim.phase === 'done')
        ) {
          lift = Math.abs(Math.sin(now / 70)) * 2.5; // a little trot on the way home
        }
        sim.lift = lift;

        let tiltTarget = 0;
        if (inside && (sim.phase === 'watch' || sim.phase === 'crouch')) {
          tiltTarget = clamp((sim.laserX - sim.x) * 0.06, -11, 11);
        } else if (sim.phase === 'pounce') {
          tiltTarget = clamp(sim.vx * 0.025, -14, 14);
        } else if (sim.pose === 'wonder') {
          tiltTarget = -9;
        }
        sim.tilt += (tiltTarget - sim.tilt) * (1 - Math.exp(-dt / 0.1));

        const crouching = now - sim.phaseAt;
        sim.wiggle =
          sim.phase === 'crouch' && crouching > 180
            ? Math.sin((crouching / 1000) * Math.PI * 12) * Math.min(1, (crouching - 180) / 150)
            : 0;
      }
      measure();

      const tracking = inside || sim.phase === 'pounce';
      const near = clamp(sim.dist / 50, 0, 1);
      const follow = 1 - Math.exp(-dt / 0.05);
      sim.lookX += ((tracking ? sim.ux * 10 * near : 0) - sim.lookX) * follow;
      sim.lookY += ((tracking ? sim.uy * 7 * near : 0) - sim.lookY) * follow;
      const hunting = !reduced && (sim.phase === 'crouch' || sim.phase === 'pounce');
      sim.dilate += ((hunting ? 1.3 : 1) - sim.dilate) * (1 - Math.exp(-dt / 0.12));

      const trailTarget = !reduced && inside ? clamp((sim.speed - 120) / 700, 0, 1) : 0;
      sim.trailLevel += (trailTarget - sim.trailLevel) * (1 - Math.exp(-dt / 0.06));

      if (!reduced && inside && sim.speed > SPARKLE_SPEED && now - sim.lastSpark > 80) {
        sim.lastSpark = now;
        sparkle();
      }
    };

    const sparkle = () => {
      const el = sparkRefs.current[sim.nextSpark++ % SPARKS];
      if (!el || typeof el.animate !== 'function') return;
      const r = bounds();
      const x = (sim.laserX * r.width) / W + (Math.random() - 0.5) * 10;
      const y = (sim.laserY * r.height) / H + (Math.random() - 0.5) * 10;
      const angle = Math.random() * Math.PI * 2;
      const distance = 10 + Math.random() * 12;
      el.animate(
        [
          { transform: `translate3d(${x}px, ${y}px, 0) scale(0.3) rotate(0deg)`, opacity: 1 },
          {
            transform: `translate3d(${x + Math.cos(angle) * distance}px, ${y + Math.sin(angle) * distance}px, 0) scale(1) rotate(80deg)`,
            opacity: 0,
          },
        ],
        { duration: 460, easing: 'ease-out' },
      );
    };

    /** Nyu-space point for a mat point, undoing her position, lift and tilt. */
    const toNyu = (x: number, y: number): [number, number] => {
      const rx = (x - sim.x) / S;
      const ry = (y - (sim.y - sim.lift)) / S;
      const a = (-sim.tilt * Math.PI) / 180;
      return [128 + rx * Math.cos(a) - ry * Math.sin(a), 220 + rx * Math.sin(a) + ry * Math.cos(a)];
    };

    const draw = () => {
      catRef.current?.setAttribute(
        'transform',
        `translate(${round(sim.x + sim.wiggle * 1.5)} ${round(sim.y)})`,
      );
      shadowRef.current?.setAttribute(
        'transform',
        `scale(${round(1 - Math.min(sim.lift, 40) / 80)} 1)`,
      );
      liftRef.current?.setAttribute('transform', `translate(0 ${round(-sim.lift)})`);
      const sx = 1 + (1 - sim.squash) * 0.6;
      bodyRef.current?.setAttribute(
        'transform',
        `rotate(${round(sim.tilt + sim.wiggle * 3.5)}) scale(${round(sx)} ${round(sim.squash)})`,
      );

      const { eyes, reach, catchL, catchR } = parts.current;
      const a = (-sim.tilt * Math.PI) / 180;
      const lookX = sim.lookX * Math.cos(a) - sim.lookY * Math.sin(a);
      const lookY = sim.lookX * Math.sin(a) + sim.lookY * Math.cos(a);
      for (const eye of eyes) {
        eye.el.setAttribute(
          'transform',
          `translate(${round(eye.cx + lookX)} ${round(148 + lookY)}) scale(${round(sim.dilate)})`,
        );
      }

      if (reach.length) {
        const [dotX, dotY] = toNyu(sim.laserX, sim.laserY);
        const dx = dotX - 128;
        const dy = dotY - 160;
        const d = Math.hypot(dx, dy) || 1;
        const r = Math.min(REACH / S, d);
        const transform = `translate(${round(128 + (dx / d) * r)} ${round(Math.min(226, 160 + (dy / d) * r))})`;
        for (const el of reach) el.setAttribute('transform', transform);
      }
      if (catchL.length || catchR.length) {
        // Both paws pin the spot where she caught it.
        let [dotX, dotY] = toNyu(sim.caughtX, sim.caughtY);
        const dx = dotX - 128;
        const dy = dotY - 160;
        const d = Math.hypot(dx, dy);
        if (d > 150) {
          dotX = 128 + (dx / d) * 150;
          dotY = 160 + (dy / d) * 150;
        }
        for (const el of catchL)
          el.setAttribute('transform', `translate(${round(dotX - 26)} ${round(dotY + 6)})`);
        for (const el of catchR)
          el.setAttribute('transform', `translate(${round(dotX + 26)} ${round(dotY + 6)})`);
      }

      const r = rect ?? (sim.pointerInside || sim.keyboard ? bounds() : null);
      if (!r) return;
      const kx = r.width / W;
      const ky = r.height / H;
      const dot = dotRef.current;
      if (dot) {
        dot.style.transform = `translate3d(${round(sim.laserX * kx)}px, ${round(sim.laserY * ky)}px, 0)`;
      }
      trailRefs.current.forEach((el, i) => {
        if (!el) return;
        const n = i + 1;
        const opacity = sim.trailLevel * (0.5 - n * 0.07);
        if (opacity < 0.01) {
          if (el.style.opacity !== '0') el.style.opacity = '0';
          return;
        }
        const x = sim.trail[n * 2] ?? sim.laserX;
        const y = sim.trail[n * 2 + 1] ?? sim.laserY;
        el.style.opacity = String(round(opacity));
        el.style.transform = `translate3d(${round(x * kx)}px, ${round(y * ky)}px, 0) scale(${round(1 - n * 0.11)})`;
      });
    };

    const tick = (now: number) => {
      const dt = clamp((now - sim.last) / 1000, 0.001, 0.05);
      sim.last = now;
      const reduced = motionReduced();
      const inside = activeRef.current && (sim.pointerInside || sim.keyboard);
      step(now, dt, inside, reduced);
      draw();

      const busy =
        inside ||
        sim.hopping ||
        sim.phase === 'crouch' ||
        sim.phase === 'pounce' ||
        sim.phase === 'caught' ||
        sim.trailLevel > 0.01 ||
        Math.abs(sim.tx - sim.x) > 0.2 ||
        Math.abs(sim.ty - sim.y) > 0.2 ||
        Math.abs(sim.vx) + Math.abs(sim.vy) > 0.5 ||
        Math.abs(sim.squash - 1) > 0.003 ||
        Math.abs(sim.squashV) > 0.01 ||
        Math.abs(sim.tilt - (sim.pose === 'wonder' ? -9 : 0)) > 0.05 ||
        Math.abs(sim.lookX) + Math.abs(sim.lookY) > 0.05 ||
        Math.abs(sim.dilate - 1) > 0.005;
      sim.frame = busy ? requestAnimationFrame(tick) : 0;
    };

    const onEnter = (event: PointerEvent) => {
      invalidate();
      const [x, y] = toPad(event.clientX, event.clientY);
      placeLaser(x, y, performance.now(), !sim.keyboard);
      sim.pointerInside = true;
      goInside();
    };

    const onMove = (event: PointerEvent) => {
      if (!sim.pointerInside) onEnter(event);
      const coalesced =
        typeof event.getCoalescedEvents === 'function' ? event.getCoalescedEvents() : [];
      const events = coalesced.length ? coalesced : [event];
      if (activeRef.current) {
        const kind = POINTER_KIND[event.pointerType] ?? 0;
        for (const e of events) {
          onEntropyRef.current(
            sample(
              e.clientX,
              e.clientY,
              e.timeStamp,
              e.movementX,
              e.movementY,
              e.pressure * 255,
              kind,
              e.buttons,
            ),
          );
        }
      }
      const [x, y] = toPad(event.clientX, event.clientY);
      placeLaser(x, y, performance.now(), false);
      if (activeRef.current) wake();
    };

    const onDown = (event: PointerEvent) => {
      invalidate();
      onMove(event);
    };

    const onLeave = () => {
      sim.pointerInside = false;
      if (sim.keyboard) showInside();
      else goIdle(true);
    };

    const KEYS: Record<string, [dx: number, dy: number, code: number]> = {
      ArrowLeft: [-1, 0, 1],
      ArrowRight: [1, 0, 2],
      ArrowUp: [0, -1, 3],
      ArrowDown: [0, 1, 4],
    };

    const startKeyboard = () => {
      if (sim.keyboard) return;
      sim.keyboard = true;
      if (sim.pointerInside) {
        showInside();
        return;
      }
      // The dot starts up beside her, on the side with more room, where she can see it.
      placeLaser(
        clamp(sim.x + (sim.x < W / 2 ? 110 : -110), 24, W - 24),
        clamp(sim.y - 120, 24, H - 40),
        performance.now(),
        true,
      );
      goInside();
    };

    const onKeyDown = (event: KeyboardEvent) => {
      const key = KEYS[event.key];
      if (!key || event.altKey || event.ctrlKey || event.metaKey) return;
      event.preventDefault();
      if (!activeRef.current) return;
      startKeyboard();
      const distance = event.shiftKey ? 32 : 12;
      const [dx, dy, code] = key;
      placeLaser(sim.laserX + dx * distance, sim.laserY + dy * distance, performance.now(), false);
      const r = bounds();
      onEntropyRef.current(
        sample(
          r.left + (sim.laserX * r.width) / W,
          r.top + (sim.laserY * r.height) / H,
          event.timeStamp,
          dx * distance,
          dy * distance,
          (event.repeat ? 0x80 : 0) | code,
          KEYBOARD_KIND,
          event.shiftKey ? 1 : 0,
        ),
      );
      wake();
    };

    const onFocus = () => {
      let keyboard = false;
      try {
        keyboard = pad.matches(':focus-visible');
      } catch {
        // Older engines without :focus-visible: the first arrow key switches it on.
      }
      if (keyboard && activeRef.current) startKeyboard();
    };

    const onBlur = () => {
      if (!sim.keyboard) return;
      sim.keyboard = false;
      if (sim.pointerInside) showInside();
      else goIdle(false);
    };

    const syncActive = () => {
      const now = performance.now();
      clearTimers();
      if (!activeRef.current) {
        sim.hopping = false;
        setPhase(progressRef.current >= 1 ? 'done' : 'calm', now);
        showInside();
      } else if (sim.pointerInside || sim.keyboard) {
        goInside();
      } else {
        goIdle(false);
      }
      wake();
    };

    const onPointerCancel = () => onLeave();
    const resize = new ResizeObserver(invalidate);
    resize.observe(pad);
    pad.addEventListener('pointerenter', onEnter);
    pad.addEventListener('pointermove', onMove);
    pad.addEventListener('pointerdown', onDown);
    pad.addEventListener('pointerleave', onLeave);
    pad.addEventListener('pointercancel', onPointerCancel);
    pad.addEventListener('keydown', onKeyDown);
    pad.addEventListener('focus', onFocus);
    pad.addEventListener('blur', onBlur);
    window.addEventListener('scroll', invalidate, { capture: true, passive: true });
    window.addEventListener('resize', invalidate);

    controls.current = { syncActive, redraw: draw };
    syncActive();

    return () => {
      controls.current = null;
      clearTimers();
      if (sim.frame) cancelAnimationFrame(sim.frame);
      sim.frame = 0;
      resize.disconnect();
      pad.removeEventListener('pointerenter', onEnter);
      pad.removeEventListener('pointermove', onMove);
      pad.removeEventListener('pointerdown', onDown);
      pad.removeEventListener('pointerleave', onLeave);
      pad.removeEventListener('pointercancel', onPointerCancel);
      pad.removeEventListener('keydown', onKeyDown);
      pad.removeEventListener('focus', onFocus);
      pad.removeEventListener('blur', onBlur);
      window.removeEventListener('scroll', invalidate, { capture: true });
      window.removeEventListener('resize', invalidate);
    };
  }, []);

  const complete = progress >= 1;
  useEffect(() => {
    controls.current?.syncActive();
  }, [active, complete]);

  // Pupils and paws come and go with the pose, and Nyu draws each part twice
  // (once as the white sticker edge), so they are looked up after every render.
  useLayoutEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const all = (selector: string) => Array.from(stage.querySelectorAll<SVGGElement>(selector));
    parts.current = {
      eyes: all('[data-part="eye"]').map((el) => ({ el, cx: Number(el.dataset.cx) })),
      reach: all('[data-part="reach"]'),
      catchL: all('[data-part="catch-l"]'),
      catchR: all('[data-part="catch-r"]'),
    };
    controls.current?.redraw();
  });

  // A small confetti burst, once, when collecting is done.
  useEffect(() => {
    if (pose !== 'done' || motionReduced()) return;
    confettiRefs.current.forEach((el, i) => {
      const piece = CONFETTI[i];
      if (!el || !piece || typeof el.animate !== 'function') return;
      const [dx, dy, rotate] = piece;
      el.animate(
        [
          { transform: `translate(${-dx}px, ${-dy}px) scale(0.3) rotate(0deg)`, opacity: 0 },
          { opacity: 1, offset: 0.12 },
          { transform: 'translate(0px, 0px) scale(1) rotate(0deg)', opacity: 1, offset: 0.5 },
          {
            transform: `translate(${dx * 0.08}px, 24px) scale(0.9) rotate(${rotate}deg)`,
            opacity: 0,
          },
        ],
        {
          duration: 1500,
          delay: i * 25,
          easing: 'cubic-bezier(0.15, 0.6, 0.35, 1)',
          fill: 'backwards',
        },
      );
    });
  }, [pose]);

  const look = LOOKS[pose];
  const pct = Math.round(clamp(progress, 0, 1) * 100);
  const style = {
    '--nyu-laser-core': NYU.blush,
    '--nyu-laser-hot': NYU.paper,
    '--nyu-laser-spark': NYU.star,
  } as CSSProperties;

  return (
    <div className={className ? `nyu-laser ${className}` : 'nyu-laser'} style={style}>
      <div
        ref={padRef}
        className="nyu-laser-pad"
        role="application"
        aria-label={label}
        aria-describedby={hint ? hintId : undefined}
        tabIndex={0}
        data-pose={pose}
        data-active={active}
        data-inside="false"
      >
        <svg
          ref={stageRef}
          className="nyu-laser-stage nyu-host nyu-blink"
          viewBox={`0 0 ${W} ${H}`}
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden
          focusable="false"
        >
          <PawPrints progress={progress} />
          <g ref={catRef} transform={`translate(${HOME.x} ${HOME.y})`}>
            <ellipse ref={shadowRef} rx="54" ry="6" fill={NYU.outline} opacity="0.14" />
            <g ref={liftRef}>
              <g ref={bodyRef}>
                <g className="nyu-laser-breathe">
                  <NyuFigure
                    mood={look.mood}
                    x={0}
                    y={-73 * S}
                    scale={S}
                    edge={NYU_EDGE}
                    behind={TAIL}
                    eyes={look.track ? TRACKING_EYES : undefined}
                    front={frontFor(pose)}
                  />
                </g>
              </g>
              {pose === 'wonder' && (
                <g transform="translate(50 -100)">
                  <g className="nyu-laser-bubble">
                    <Sticker edge={11}>
                      <path
                        d="M-7 -5 q0 -9 9 -9 q9 0 9 8 q0 6 -8 9 v4"
                        fill="none"
                        stroke={NYU.outline}
                        strokeWidth={5}
                      />
                      <circle cx="3" cy="15" r="3.2" fill={NYU.outline} />
                    </Sticker>
                  </g>
                </g>
              )}
              {pose === 'sleep' && (
                <g transform="translate(42 -92)">
                  <g className="nyu-laser-z">
                    <Sticker edge={9}>
                      <path
                        d="M0 -10 h9 l-9 10 h9"
                        fill="none"
                        stroke={NYU.outline}
                        strokeWidth={3.5}
                      />
                    </Sticker>
                  </g>
                  <g className="nyu-laser-z nyu-laser-z-late">
                    <Sticker edge={9}>
                      <path
                        d="M14 -30 h12 l-12 13 h12"
                        fill="none"
                        stroke={NYU.outline}
                        strokeWidth={4}
                      />
                    </Sticker>
                  </g>
                </g>
              )}
              {pose === 'done' && (
                <Sticker edge={10}>
                  <g className="nyu-laser-twinkle">
                    <Star x={-78} y={-74} r={9} />
                  </g>
                  <g className="nyu-laser-twinkle nyu-laser-twinkle-late">
                    <Star x={82} y={-92} r={11} />
                  </g>
                </Sticker>
              )}
            </g>
          </g>
          <g>
            {BURST.map(({ item }, i) => (
              <g
                key={i}
                ref={(el) => {
                  burstRefs.current[i] = el;
                }}
              >
                <g className="nyu-laser-burst">{item}</g>
              </g>
            ))}
          </g>
          {pose === 'done' && (
            <g>
              {CONFETTI.map(([dx, dy, rotate, fill], i) => (
                <g key={i} transform={`translate(${HEAD.x + dx} ${HEAD.y + dy})`}>
                  <g
                    className="nyu-laser-confetto"
                    ref={(el) => {
                      confettiRefs.current[i] = el;
                    }}
                  >
                    <Sticker edge={10}>
                      <rect
                        x="-7"
                        y="-4"
                        width="14"
                        height="8"
                        rx="2"
                        transform={`rotate(${rotate})`}
                        fill={fill}
                        stroke={NYU.outline}
                        strokeWidth={3}
                      />
                    </Sticker>
                  </g>
                </g>
              ))}
            </g>
          )}
        </svg>
        <div className="nyu-laser-beam" aria-hidden>
          {Array.from({ length: TRAIL - 1 }, (_, i) => (
            <div
              key={`trail-${i}`}
              className="nyu-laser-trail"
              ref={(el) => {
                trailRefs.current[i] = el;
              }}
            />
          ))}
          {Array.from({ length: SPARKS }, (_, i) => (
            <div
              key={`spark-${i}`}
              className={i % 2 ? 'nyu-laser-spark nyu-laser-spark-gold' : 'nyu-laser-spark'}
              ref={(el) => {
                sparkRefs.current[i] = el;
              }}
            />
          ))}
          <div ref={dotRef} className="nyu-laser-dot" />
        </div>
      </div>
      <div
        className="nyu-laser-sr"
        role="progressbar"
        aria-label={progressLabel}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        aria-valuetext={`${pct} %`}
      />
      {hint && (
        <p id={hintId} className="nyu-laser-hint">
          {hint}
        </p>
      )}
    </div>
  );
}
