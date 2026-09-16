/**
 * Nyu, the terminal cat.
 *
 * Same cat as in UwUMail — the envelope is a terminal window here, with the
 * screen as the face. The colours are fixed artwork, not theme tokens: a
 * sticker looks like itself in dark mode too, and the white die-cut edge is
 * what keeps the outlines readable on a dark ground.
 */

export const NYU = {
  outline: '#4B1D3F',
  body: '#FF6FA6',
  screen: '#FFB8D3',
  blush: '#FF4D8D',
  edge: '#FFFFFF',
} as const;

type NyuProps = {
  size?: number;
  /** Blinking is on by default and stops on its own when motion is reduced. */
  blink?: boolean;
  title?: string;
};

export function Nyu({ size = 96, blink = true, title = 'Nyu' }: NyuProps) {
  return (
    <svg
      viewBox="0 0 256 256"
      width={size}
      height={size}
      role="img"
      aria-label={title}
      focusable="false"
    >
      <g
        fill={NYU.edge}
        stroke={NYU.edge}
        strokeWidth={20}
        strokeLinejoin="round"
        strokeLinecap="round"
      >
        <path d="M62 88 L80 36 L114 88 Z" />
        <path d="M142 88 L176 36 L194 88 Z" />
        <rect x={28} y={74} width={200} height={146} rx={24} />
      </g>

      <g stroke={NYU.outline} strokeWidth={9} strokeLinejoin="round">
        <path d="M62 88 L80 36 L114 88 Z" fill={NYU.body} />
        <path d="M142 88 L176 36 L194 88 Z" fill={NYU.body} />
      </g>
      <path d="M76 82 L84 52 L100 82 Z" fill={NYU.screen} />
      <path d="M156 82 L172 52 L180 82 Z" fill={NYU.screen} />

      <rect
        x={28}
        y={74}
        width={200}
        height={146}
        rx={24}
        fill={NYU.body}
        stroke={NYU.outline}
        strokeWidth={9}
      />

      <circle cx={52} cy={98} r={5.5} fill={NYU.screen} />
      <circle cx={70} cy={98} r={5.5} fill={NYU.screen} />
      <circle cx={88} cy={98} r={5.5} fill={NYU.screen} />

      <rect
        x={44}
        y={116}
        width={168}
        height={88}
        rx={16}
        fill={NYU.screen}
        stroke={NYU.outline}
        strokeWidth={7}
      />

      <ellipse cx={74} cy={168} rx={12} ry={7.5} fill={NYU.blush} opacity={0.5} />
      <ellipse cx={182} cy={168} rx={12} ry={7.5} fill={NYU.blush} opacity={0.5} />

      <g
        fill="none"
        stroke={NYU.outline}
        strokeWidth={8}
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <g className={blink ? 'nyu-eyes' : undefined}>
          <path d="M88 142 Q102 160 116 142" />
          <path d="M140 142 Q154 160 168 142" />
        </g>
        <path d="M112 170 L120 182 L128 170 L136 182 L144 170" />
      </g>
    </svg>
  );
}
