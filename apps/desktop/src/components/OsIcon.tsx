/**
 * Little stickers for the operating system a server runs, shown next to hosts
 * once UwUSSH has detected it after connecting.
 *
 * Every icon is the same tile with Nyu's cat ears and one simple glyph that
 * evokes the system without copying its logo. Like Nyu, the colours are fixed
 * artwork rather than theme tokens, and a white die-cut edge keeps dark tiles
 * readable on a dark sidebar.
 */

import type { ReactNode } from 'react';
import { N_, t, useLanguage } from '../lib/i18n';
import { NYU } from './nyu/Nyu';

export type OsId =
  | 'ubuntu'
  | 'debian'
  | 'fedora'
  | 'redhat'
  | 'centos'
  | 'rocky'
  | 'alma'
  | 'arch'
  | 'alpine'
  | 'suse'
  | 'raspberry'
  | 'proxmox'
  | 'mint'
  | 'kali'
  | 'nixos'
  | 'freebsd'
  | 'macos'
  | 'windows'
  | 'cisco'
  | 'mikrotik'
  | 'synology'
  | 'openwrt'
  | 'linux';

export const OS_LABELS: Record<OsId, string> = {
  ubuntu: 'Ubuntu',
  debian: 'Debian',
  fedora: 'Fedora',
  redhat: 'Red Hat',
  centos: 'CentOS',
  rocky: 'Rocky Linux',
  alma: 'AlmaLinux',
  arch: 'Arch Linux',
  alpine: 'Alpine',
  suse: 'openSUSE',
  raspberry: 'Raspberry Pi',
  proxmox: 'Proxmox VE',
  mint: 'Linux Mint',
  kali: 'Kali Linux',
  nixos: 'NixOS',
  freebsd: 'FreeBSD',
  macos: 'macOS',
  windows: 'Windows',
  cisco: 'Cisco',
  mikrotik: 'MikroTik',
  synology: 'Synology',
  openwrt: 'OpenWrt',
  linux: 'Linux',
};

export const OS_IDS: readonly OsId[] = Object.keys(OS_LABELS) as OsId[];

/** For values that come from outside TypeScript, like the detected OS stored with a host. */
export function isOsId(value: unknown): value is OsId {
  return typeof value === 'string' && Object.hasOwn(OS_LABELS, value);
}

const UNKNOWN_LABEL = N_('Unbekanntes System');

// All drawing happens on a 40 × 40 canvas. The outline is 2.4 units, which is
// 1.1–1.2 px at list size (18–20 px) and grows with the icon like a sticker.
const INK = NYU.outline;
const WHITE = NYU.paper;
const LINE = 2.4;
const THIN = 1.8;
/** 1.3 units of white beyond the outline: visible on a dark ground without eating the tile. */
const EDGE = LINE + 2.6;

const TILE = { x: 3, y: 9.5, width: 34, height: 27.5, rx: 8 } as const;

// Same ear as Nyu's, scaled to the tile: the tip leans outward and the base
// hides behind the tile's top edge.
const EARS = [
  { side: 'l', d: 'M8.5 13 L11.5 3.8 L18 10.5 Z', inner: 'M11.2 10.4 L12.4 6.6 L15.2 10.4 Z' },
  { side: 'r', d: 'M31.5 13 L28.5 3.8 L22 10.5 Z', inner: 'M28.8 10.4 L27.6 6.6 L24.8 10.4 Z' },
] as const;

function Ears({ fill }: { fill: string }) {
  return (
    <>
      {EARS.map(({ side, d, inner }) => (
        // `nyu-ear` lets the ears twitch on hover exactly like Nyu's (nyu.css).
        <g key={side} className={`nyu-ear nyu-ear-${side}`}>
          <path d={d} fill={WHITE} stroke={WHITE} strokeWidth={EDGE} />
          <path d={d} fill={fill} stroke={INK} strokeWidth={LINE} />
          <path d={inner} fill={WHITE} opacity={0.4} />
        </g>
      ))}
    </>
  );
}

/**
 * The tile with its die-cut edge. The edge goes down before the ears so it
 * never cuts across them, and the outline is drawn again over the glyph so a
 * glyph can run into the frame, like a hill standing on the bottom edge.
 */
function Tile({ fill, ears, children }: { fill: string; ears: boolean; children: ReactNode }) {
  return (
    <g strokeLinecap="round" strokeLinejoin="round">
      <rect {...TILE} fill={WHITE} stroke={WHITE} strokeWidth={EDGE} />
      {ears && <Ears fill={fill} />}
      <rect {...TILE} fill={fill} />
      {children}
      <rect {...TILE} fill="none" stroke={INK} strokeWidth={LINE} />
    </g>
  );
}

/** Dot eyes and a small `ω` mouth, centred between the eyes; cheeks are optional. */
function Face({
  x,
  y,
  gap = 5,
  blush = false,
}: {
  x: number;
  y: number;
  gap?: number;
  blush?: boolean;
}) {
  return (
    <g>
      {blush && (
        <g fill={NYU.blush} opacity={0.5}>
          <ellipse cx={x - gap / 2 - 1.6} cy={y + 2.4} rx={1.6} ry={1.05} />
          <ellipse cx={x + gap / 2 + 1.6} cy={y + 2.4} rx={1.6} ry={1.05} />
        </g>
      )}
      <circle cx={x - gap / 2} cy={y} r={1.25} fill={INK} />
      <circle cx={x + gap / 2} cy={y} r={1.25} fill={INK} />
      <path
        d={`M${x - 2} ${y + 2} q1 1.3 2 0 q1 1.3 2 0`}
        fill="none"
        stroke={INK}
        strokeWidth={1.4}
      />
    </g>
  );
}

const outlined = { stroke: INK, strokeWidth: THIN } as const;

function ring(count: number, radius: number, startDeg: number) {
  return Array.from({ length: count }, (_, i) => {
    const rad = ((startDeg + (360 / count) * i) * Math.PI) / 180;
    return [20 + radius * Math.cos(rad), 23.5 + radius * Math.sin(rad)] as const;
  });
}

const FRIENDS = ring(3, 7, -90);
const PETALS = ring(5, 5.2, -90);
const BLADE = 'M20 23.5 V14 Q25.5 14.5 26 20.5 Z';
const TRIANGLE = 'M20 14.5 L29 31 H11 Z';
const SNOWFLAKE = 'M20 15 V32 M12.64 19.25 L27.36 27.75 M27.36 19.25 L12.64 27.75';

type Icon = { tile: string; glyph: ReactNode };

const ICONS: Record<OsId, Icon> = {
  // Three friends holding hands in a ring.
  ubuntu: {
    tile: '#F07A3F',
    glyph: (
      <>
        <circle cx={20} cy={23.5} r={7} fill="none" stroke={WHITE} strokeWidth={2.8} />
        <g fill={WHITE} {...outlined}>
          {FRIENDS.map(([cx, cy]) => (
            <circle key={cx} cx={cx} cy={cy} r={2.9} />
          ))}
        </g>
      </>
    ),
  },
  // A swirl, opening to the left.
  debian: {
    tile: '#DD3A6E',
    glyph: (
      <path
        d="M22.5 24.75 A2.5 2.5 0 0 0 17.5 24.75 A5 5 0 0 0 27.5 24.75 A7.5 7.5 0 0 0 12.5 24.75"
        fill="none"
        stroke={WHITE}
        strokeWidth={2.8}
      />
    ),
  },
  // A speech bubble saying "f".
  fedora: {
    tile: '#3F76CC',
    glyph: (
      <>
        <path d="M14.84 28.16 A8 8 0 1 1 17.76 30.02 L13 31.8 Z" fill={WHITE} {...outlined} />
        <path
          d="M19.8 28 V20.6 a2.6 2.6 0 0 1 2.6 -2.6 h0.6 M17.2 22.6 h5.4"
          fill="none"
          stroke="#2D5BA8"
          strokeWidth={2.8}
        />
      </>
    ),
  },
  // The tile wears a little hat.
  redhat: {
    tile: '#E5484D',
    glyph: (
      <>
        <path
          d="M13.5 21.5 C13.5 16 14.8 13.8 17 13.8 C18.3 13.8 19 14.8 20 14.8 C21 14.8 21.7 13.8 23 13.8 C25.2 13.8 26.5 16 26.5 21.5 Z"
          fill="#A8141F"
          {...outlined}
        />
        <path d="M14.4 19.6 H25.6" stroke={WHITE} strokeWidth={2} strokeLinecap="butt" />
        <path
          d="M8.8 21.8 C11 24.8 29 24.8 31.2 21.8 C29 19.8 11 19.8 8.8 21.8 Z"
          fill="#A8141F"
          {...outlined}
        />
        <Face x={20} y={27.5} gap={6} />
      </>
    ),
  },
  // A pinwheel.
  centos: {
    tile: '#8E5BB5',
    glyph: (
      <g {...outlined} strokeWidth={1.6}>
        {[NYU.star, '#8FE0B0', '#9ED8FF', NYU.body].map((fill, i) => (
          <path key={fill} d={BLADE} fill={fill} transform={`rotate(${i * 90} 20 23.5)`} />
        ))}
        <circle cx={20} cy={23.5} r={1.4} fill={INK} stroke="none" />
      </g>
    ),
  },
  // A faceted rock on the bottom edge, half pet rock and half mountain. The
  // corners keep it from reading as a ghost.
  rocky: {
    tile: '#2BAE78',
    glyph: (
      <>
        <path
          d="M7.5 36.5 L10 25.5 L15.5 19 L21.5 16 L27 19.5 L31 25 L32.5 36.5 Z"
          fill="#D5E2DA"
          {...outlined}
        />
        <Face x={20.5} y={26.5} gap={5.6} blush />
      </>
    ),
  },
  // A flower in the sun.
  alma: {
    tile: '#3A8FD9',
    glyph: (
      <>
        {/* Outlines first, then the fills over them, so the petals merge into one flower. */}
        <g fill={NYU.star} {...outlined}>
          {PETALS.map(([cx, cy]) => (
            <circle key={cx} cx={cx} cy={cy} r={3.2} />
          ))}
        </g>
        <g fill={NYU.star}>
          {PETALS.map(([cx, cy]) => (
            <circle key={cx} cx={cx} cy={cy} r={3.2} />
          ))}
        </g>
        <circle cx={20} cy={23.5} r={3.4} fill="#FF9F5A" {...outlined} />
      </>
    ),
  },
  // A soft triangle with a face.
  arch: {
    tile: '#3AA5DC',
    glyph: (
      <>
        {/* Stroked in its own colour to round the corners, with the ink a little wider below. */}
        <path d={TRIANGLE} fill={INK} stroke={INK} strokeWidth={2.4 + 2 * THIN} />
        <path d={TRIANGLE} fill={WHITE} stroke={WHITE} strokeWidth={2.4} />
        <Face x={20} y={25.5} gap={4.6} />
      </>
    ),
  },
  // Two mountains, the front one smiling.
  alpine: {
    tile: '#1D6594',
    glyph: (
      <>
        <path d="M18.5 31 L25.5 19.5 L32 31 Z" fill={NYU.sky} {...outlined} />
        <path d="M8 31 L16 17 L24.5 31 Z" fill={WHITE} {...outlined} />
        <Face x={16.2} y={26} gap={3.8} />
      </>
    ),
  },
  // A round chameleon with one big eye.
  suse: {
    tile: '#6DB33F',
    glyph: (
      <>
        <path
          d="M9.5 30.5 C9 22.5 14 16 21 16 C27.5 16 31 20.5 31 25 C31 29 27.5 30.5 22 30.5 Z"
          fill="#DDF3C4"
          {...outlined}
        />
        <circle cx={23.5} cy={21.8} r={3.6} fill={WHITE} {...outlined} />
        <circle cx={24.4} cy={21.8} r={1.5} fill={INK} />
        <path d="M24.5 26.8 Q27 28 29 26.3" fill="none" stroke={INK} strokeWidth={1.4} />
      </>
    ),
  },
  // A raspberry with a face.
  raspberry: {
    tile: '#C8234F',
    glyph: (
      <>
        <path
          d="M20 17.5 C26.5 17.5 29.5 21.5 28 26.5 C26.8 30.8 23 33 20 33 C17 33 13.2 30.8 12 26.5 C10.5 21.5 13.5 17.5 20 17.5 Z"
          fill="#FF8FB3"
          {...outlined}
        />
        <g fill="#7DD47F" {...outlined}>
          <path d="M20 18.5 C17.5 13.5 13.5 13 12 14.8 C13 17.8 16.5 19.5 20 18.5 Z" />
          <path d="M20 18.5 C22.5 13.5 26.5 13 28 14.8 C27 17.8 23.5 19.5 20 18.5 Z" />
        </g>
        <Face x={20} y={24.5} gap={6} />
      </>
    ),
  },
  // The two halves of an X, pulled apart into a squeezed >ω< face.
  proxmox: {
    tile: '#F28A2E',
    glyph: (
      <g fill="none" stroke="#3A2530">
        <path d="M11.5 16.5 L17.5 22 L11.5 27.5 M28.5 16.5 L22.5 22 L28.5 27.5" strokeWidth={3.2} />
        <path d="M16.5 29.5 q1.75 2.2 3.5 0 q1.75 2.2 3.5 0" strokeWidth={2} />
      </g>
    ),
  },
  // A leaf.
  mint: {
    tile: '#7CC655',
    glyph: (
      <>
        <path d="M11 31 C11 21 17 15 29 15 C29 26 22 31 11 31 Z" fill={WHITE} {...outlined} />
        <path d="M12 30 L22.5 20.5" fill="none" stroke="#7CC655" strokeWidth={1.8} />
      </>
    ),
  },
  // A little dragon in profile, neck rising from the bottom edge, one horn back.
  kali: {
    tile: '#2A3766',
    glyph: (
      <>
        <path
          d="M16 17 Q12.5 14.5 11 11.8 Q16.5 12.5 20 14.5 Z M14.2 27 L10.6 25.6 L14.4 23.2 Z"
          fill={NYU.sky}
          {...outlined}
        />
        <path
          d="M8.5 36.5 C9 30.5 14.5 29.5 14.5 25 C14.5 21.5 12.5 20.5 14.5 17.5 C16.5 14.5 22 13.5 26.5 14.8 C30 15.8 32.5 18 32 20.5 C31.6 22.5 29.5 23 27.5 23 L23 23 C20.5 23 20.5 25 21 27.5 C21.8 31.5 18 34 16.5 36.5 Z"
          fill={WHITE}
          {...outlined}
        />
        <circle cx={24} cy={18.2} r={1.25} fill={INK} />
      </>
    ),
  },
  // A snowflake.
  nixos: {
    tile: '#6FA9DE',
    glyph: (
      // Lines have no outline of their own, so the ink goes underneath, wider.
      <g fill="none">
        <path d={SNOWFLAKE} stroke={INK} strokeWidth={2.8 + 2 * THIN} />
        <path d={SNOWFLAKE} stroke={WHITE} strokeWidth={2.8} />
      </g>
    ),
  },
  // A little devil blob.
  freebsd: {
    tile: '#D4353B',
    glyph: (
      <>
        <g fill="#8E1B27" {...outlined}>
          <path d="M14.2 21 Q11.5 16 13.2 13.2 Q16 15.5 18.5 18.2 Z" />
          <path d="M25.8 21 Q28.5 16 26.8 13.2 Q24 15.5 21.5 18.2 Z" />
        </g>
        <circle cx={20} cy={25} r={7.5} fill="#FFD0D3" {...outlined} />
        <Face x={20} y={24.5} gap={5.4} />
      </>
    ),
  },
  // An apple with a leaf and a face.
  macos: {
    tile: '#C5CBD3',
    glyph: (
      <>
        <path
          d="M20 19 C16.5 16.8 11.5 18.3 11.5 24.2 C11.5 29.2 14.8 32.5 17.3 32.5 C18.6 32.5 19.2 31.9 20 31.9 C20.8 31.9 21.4 32.5 22.7 32.5 C25.2 32.5 28.5 29.2 28.5 24.2 C28.5 18.3 23.5 16.8 20 19 Z"
          fill={WHITE}
          {...outlined}
        />
        <path
          d="M20.3 18 C20 15 22 13 25.3 13 C25.5 16 23.5 18 20.3 18 Z"
          fill="#8FD694"
          {...outlined}
        />
        <Face x={20} y={24.8} gap={5.6} blush />
      </>
    ),
  },
  // Four rounded panes.
  windows: {
    tile: '#2F86DE',
    glyph: (
      <g fill={WHITE}>
        <rect x={11.5} y={15.5} width={7.5} height={7} rx={1.8} />
        <rect x={21} y={15.5} width={7.5} height={7} rx={1.8} />
        <rect x={11.5} y={24.5} width={7.5} height={7} rx={1.8} />
        <rect x={21} y={24.5} width={7.5} height={7} rx={1.8} />
      </g>
    ),
  },
  // Bars like a bridge: two towers, low in the middle.
  cisco: {
    tile: '#169FBE',
    glyph: (
      <path
        d="M10 21 V26 M15 17 V30 M20 20 V27 M25 17 V30 M30 21 V26"
        stroke={WHITE}
        strokeWidth={3}
      />
    ),
  },
  // A bold "M".
  mikrotik: {
    tile: '#3B4656',
    glyph: (
      <path d="M12 30.5 V17 L20 25 L28 17 V30.5" fill="none" stroke={WHITE} strokeWidth={3.4} />
    ),
  },
  // A small NAS with a drive slot and a face.
  synology: {
    tile: '#5E6E82',
    glyph: (
      <>
        <rect x={12} y={14.5} width={16} height={18} rx={3} fill={WHITE} {...outlined} />
        <path d="M15.5 18.5 H24.5" stroke="#5E6E82" strokeWidth={1.8} />
        <Face x={20} y={24.5} gap={5.4} blush />
      </>
    ),
  },
  // A router with two antennas and a face.
  openwrt: {
    tile: '#1FA8D6',
    glyph: (
      <>
        <path d="M13.5 22 L12 15.5 M26.5 22 L28 15.5" stroke={INK} strokeWidth={2.2} />
        <g fill={WHITE} {...outlined}>
          <circle cx={12} cy={15} r={1.8} />
          <circle cx={28} cy={15} r={1.8} />
          <rect x={10} y={21.5} width={20} height={10} rx={3} />
        </g>
        <Face x={20} y={25.3} gap={6} />
      </>
    ),
  },
  // A round penguin.
  linux: {
    tile: NYU.star,
    glyph: (
      <>
        <g fill="#FF9F43" {...outlined} strokeWidth={1.4}>
          <ellipse cx={16.8} cy={33} rx={2.4} ry={1.3} />
          <ellipse cx={23.2} cy={33} rx={2.4} ry={1.3} />
        </g>
        <ellipse cx={20} cy={24.3} rx={8} ry={9} fill="#2E2433" {...outlined} />
        <ellipse cx={20} cy={26.3} rx={5.3} ry={6.2} fill={WHITE} />
        <circle cx={18} cy={23.2} r={1.2} fill={INK} />
        <circle cx={22} cy={23.2} r={1.2} fill={INK} />
        <path d="M18.6 25 H21.4 L20 26.8 Z" fill="#FF9F43" />
      </>
    ),
  },
};

// Unknown systems get a calm prompt on a neutral tile, and no ears: that
// combination would read as Nyu herself.
const UNKNOWN: Icon = {
  tile: '#CFC4D8',
  glyph: (
    <path d="M13 18.5 L18 23 L13 27.5 M20.5 28 H27" fill="none" stroke={INK} strokeWidth={2.8} />
  ),
};

type OsIconProps = {
  os: string | null | undefined;
  size?: number;
  /** Accessible name; defaults to the system's name. An empty string hides the icon from screen readers. */
  title?: string;
  className?: string;
};

/**
 * Drawn for 18–20 px in the host list and 28–40 px in headers. `os` null or
 * unknown → a neutral little terminal glyph.
 */
export function OsIcon({ os, size = 20, title, className }: OsIconProps) {
  useLanguage();
  const id = isOsId(os) ? os : null;
  const icon = id ? ICONS[id] : UNKNOWN;
  const label = title ?? (id ? OS_LABELS[id] : t(UNKNOWN_LABEL));
  return (
    <svg
      viewBox="0 0 40 40"
      width={size}
      height={size}
      className={className ? `nyu-host ${className}` : 'nyu-host'}
      focusable="false"
      role={label ? 'img' : undefined}
      aria-label={label || undefined}
      aria-hidden={label ? undefined : true}
    >
      <Tile fill={icon.tile} ears={id !== null}>
        {icon.glyph}
      </Tile>
    </svg>
  );
}
