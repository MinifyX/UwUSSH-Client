/**
 * What is printed on the keys, per keyboard layout, for the keyboard in the
 * shortcut settings.
 *
 * A shortcut is a physical key (`KeyboardEvent.code`), so the picture has to
 * say what that key is on the keyboard in front of the user: KeyY is the Z on
 * a German one. Each layout is the 48 keys that differ, row by row, without
 * Shift. Where the webview can tell (`navigator.keyboard`, in WebView2) the
 * real layout is read from the system instead and these tables only fill in.
 *
 * This file imports nothing, so `node --test` can run its tests as they are.
 */

/** ANSI has the wide Enter, ISO the tall one and a key next to the left Shift, JIS and ABNT one more. */
export type Geometry = 'ansi' | 'iso' | 'jis' | 'abnt';

export type Layout = {
  id: string;
  /** In its own language, as system settings list it. */
  name: string;
  geometry: Geometry;
  /** The four rows of keys that carry characters, in {@link ROW_CODES}' order. */
  rows: readonly [string, string, string, string];
  /** Keys outside the four rows: JIS's ¥ and ろ, ABNT's second slash. */
  extra?: Readonly<Record<string, string>>;
};

/** The codes behind a layout's `rows`, one character each. */
export const ROW_CODES: readonly (readonly string[])[] = [
  [
    'Backquote',
    'Digit1',
    'Digit2',
    'Digit3',
    'Digit4',
    'Digit5',
    'Digit6',
    'Digit7',
    'Digit8',
    'Digit9',
    'Digit0',
    'Minus',
    'Equal',
  ],
  [
    'KeyQ',
    'KeyW',
    'KeyE',
    'KeyR',
    'KeyT',
    'KeyY',
    'KeyU',
    'KeyI',
    'KeyO',
    'KeyP',
    'BracketLeft',
    'BracketRight',
  ],
  [
    'KeyA',
    'KeyS',
    'KeyD',
    'KeyF',
    'KeyG',
    'KeyH',
    'KeyJ',
    'KeyK',
    'KeyL',
    'Semicolon',
    'Quote',
    'Backslash',
  ],
  [
    'IntlBackslash',
    'KeyZ',
    'KeyX',
    'KeyC',
    'KeyV',
    'KeyB',
    'KeyN',
    'KeyM',
    'Comma',
    'Period',
    'Slash',
  ],
];

const layout = (
  id: string,
  name: string,
  geometry: Geometry,
  rows: Layout['rows'],
  extra?: Layout['extra'],
): Layout => ({ id, name, geometry, rows, extra });

/** By id; the ids are the ones Linux uses (xkb), which the backend reports. */
export const LAYOUTS: readonly Layout[] = [
  layout('us', 'English (US)', 'ansi', [
    '`1234567890-=',
    'qwertyuiop[]',
    "asdfghjkl;'\\",
    '\\zxcvbnm,./',
  ]),
  layout('gb', 'English (UK)', 'iso', [
    '`1234567890-=',
    'qwertyuiop[]',
    "asdfghjkl;'#",
    '\\zxcvbnm,./',
  ]),
  layout('de', 'Deutsch', 'iso', ['^1234567890ß´', 'qwertzuiopü+', 'asdfghjklöä#', '<yxcvbnm,.-']),
  layout('ch', 'Deutsch (Schweiz)', 'iso', [
    "§1234567890'^",
    'qwertzuiopü¨',
    'asdfghjklöä$',
    '<yxcvbnm,.-',
  ]),
  layout('ch-fr', 'Français (Suisse)', 'iso', [
    "§1234567890'^",
    'qwertzuiopè¨',
    'asdfghjkléà$',
    '<yxcvbnm,.-',
  ]),
  layout('fr', 'Français', 'iso', [
    '²&é"\'(-è_çà)=',
    'azertyuiop^$',
    'qsdfghjklmù*',
    '<wxcvbn,;:!',
  ]),
  layout('be', 'Belge', 'iso', ['²&é"\'(§è!çà)-', 'azertyuiop^$', 'qsdfghjklmùµ', '<wxcvbn,;:=']),
  layout('es', 'Español', 'iso', ["º1234567890'¡", 'qwertyuiop`+', 'asdfghjklñ´ç', '<zxcvbnm,.-']),
  layout('latam', 'Español (Latinoamérica)', 'iso', [
    "|1234567890'¿",
    'qwertyuiop´+',
    'asdfghjklñ{}',
    '<zxcvbnm,.-',
  ]),
  layout('it', 'Italiano', 'iso', [
    "\\1234567890'ì",
    'qwertyuiopè+',
    'asdfghjklòàù',
    '<zxcvbnm,.-',
  ]),
  layout('pt', 'Português', 'iso', [
    "\\1234567890'«",
    'qwertyuiop+´',
    'asdfghjklçº~',
    '<zxcvbnm,.-',
  ]),
  layout(
    'br',
    'Português (Brasil)',
    'abnt',
    ["'1234567890-=", 'qwertyuiop´[', 'asdfghjklç~]', '\\zxcvbnm,.;'],
    {
      IntlRo: '/',
    },
  ),
  layout('nl', 'Nederlands', 'iso', [
    '@1234567890/°',
    'qwertyuiop¨*',
    'asdfghjkl+´<',
    ']zxcvbnm,.-',
  ]),
  layout('se', 'Svenska', 'iso', ['§1234567890+´', 'qwertyuiopå¨', "asdfghjklöä'", '<zxcvbnm,.-']),
  layout('fi', 'Suomi', 'iso', ['§1234567890+´', 'qwertyuiopå¨', "asdfghjklöä'", '<zxcvbnm,.-']),
  layout('no', 'Norsk', 'iso', ['|1234567890+\\', 'qwertyuiopå¨', "asdfghjkløæ'", '<zxcvbnm,.-']),
  layout('dk', 'Dansk', 'iso', ['½1234567890+´', 'qwertyuiopå¨', "asdfghjklæø'", '<zxcvbnm,.-']),
  layout('is', 'Íslenska', 'iso', ['°1234567890ö-', "qwertyuiopð'", 'asdfghjklæ´+', '<zxcvbnm,.þ']),
  layout('ee', 'Eesti', 'iso', ['ˇ1234567890+´', 'qwertyuiopüõ', "asdfghjklöä'", '<zxcvbnm,.-']),
  layout('pl', 'Polski', 'ansi', [
    '`1234567890-=',
    'qwertyuiop[]',
    "asdfghjkl;'\\",
    '\\zxcvbnm,./',
  ]),
  layout('cz', 'Čeština', 'iso', [';+ěščřžýáíé=´', 'qwertzuiopú)', 'asdfghjklů§¨', '\\yxcvbnm,.-']),
  layout('sk', 'Slovenčina', 'iso', [
    ';+ľščťžýáíé=´',
    'qwertzuiopúä',
    'asdfghjklô§ň',
    '&yxcvbnm,.-',
  ]),
  layout('hu', 'Magyar', 'iso', ['0123456789öüó', 'qwertzuiopőú', 'asdfghjkléáű', 'íyxcvbnm,.-']),
  layout('hr', 'Hrvatski', 'iso', ["¸1234567890'+", 'qwertzuiopšđ', 'asdfghjklčćž', '<yxcvbnm,.-']),
  layout('si', 'Slovenščina', 'iso', [
    "¸1234567890'+",
    'qwertzuiopšđ',
    'asdfghjklčćž',
    '<yxcvbnm,.-',
  ]),
  layout('ro', 'Română', 'iso', ['„1234567890-=', 'qwertyuiopăî', 'asdfghjklșțâ', '\\zxcvbnm,./']),
  layout('tr', 'Türkçe', 'iso', ['"1234567890*-', 'qwertyuıopğü', 'asdfghjklşi,', '<zxcvbnmöç.']),
  layout('gr', 'Ελληνικά', 'iso', [
    '`1234567890-=',
    ';ςερτυθιοπ[]',
    "ασδφγηξκλ΄'\\",
    '\\ζχψωβνμ,./',
  ]),
  layout('ru', 'Русский', 'ansi', [
    'ё1234567890-=',
    'йцукенгшщзхъ',
    'фывапролджэ\\',
    '\\ячсмитьбю.',
  ]),
  layout('ua', 'Українська', 'ansi', [
    "'1234567890-=",
    'йцукенгшщзхї',
    'фівапролджє\\',
    'ґячсмитьбю.',
  ]),
  layout('il', 'עברית', 'ansi', [';1234567890-=', "/'קראטוןםפ][", 'שדגכעיחלךף,\\', '\\זסבהנמצתץ.']),
  layout('jp', '日本語', 'jis', ['半1234567890-^', 'qwertyuiop@[', 'asdfghjkl;:]', ' zxcvbnm,./'], {
    IntlYen: '¥',
    IntlRo: '\\',
  }),
  layout('kr', '한국어', 'ansi', [
    '`1234567890-=',
    'qwertyuiop[]',
    "asdfghjkl;'\\",
    '\\zxcvbnm,./',
  ]),
  layout('us-dvorak', 'Dvorak', 'ansi', [
    '`1234567890[]',
    "',.pyfgcrl/=",
    'aoeuidhtns-\\',
    '\\;qjkxbmwvz',
  ]),
  layout('us-colemak', 'Colemak', 'ansi', [
    '`1234567890-=',
    'qwfpgjluy;[]',
    "arstdhneio'\\",
    '\\zxcvbkm,./',
  ]),
  layout('us-workman', 'Workman', 'ansi', [
    '`1234567890-=',
    'qdrwbjfup;[]',
    "ashtgyneoi'\\",
    '\\zxmcvkl,./',
  ]),
];

/** Other names for the same keys: countries that share a layout, and how Windows and macOS say it. */
const ALIASES: Record<string, string> = {
  en: 'us',
  'en-us': 'us',
  'us-intl': 'us',
  'us-altgr-intl': 'us',
  au: 'us',
  nz: 'us',
  ca: 'us',
  'en-gb': 'gb',
  uk: 'gb',
  ie: 'gb',
  at: 'de',
  'de-at': 'de',
  'de-de': 'de',
  'de-ch': 'ch',
  'ch-de': 'ch',
  li: 'ch',
  'fr-ch': 'ch-fr',
  lu: 'ch-fr',
  'fr-fr': 'fr',
  'fr-be': 'be',
  'nl-be': 'be',
  'es-es': 'es',
  mx: 'latam',
  'es-mx': 'latam',
  'es-419': 'latam',
  'it-it': 'it',
  'pt-pt': 'pt',
  'pt-br': 'br',
  sv: 'se',
  'sv-se': 'se',
  nb: 'no',
  nn: 'no',
  da: 'dk',
  et: 'ee',
  cs: 'cz',
  sl: 'si',
  rs: 'hr',
  'rs-latin': 'hr',
  ba: 'hr',
  el: 'gr',
  uk_ua: 'ua',
  he: 'il',
  ja: 'jp',
  ko: 'kr',
  dvorak: 'us-dvorak',
  colemak: 'us-colemak',
  workman: 'us-workman',
};

/**
 * The layout a name stands for: an xkb id (`de`, `ch-fr`, `us-dvorak`), or a
 * language tag (`de-CH`). `null` for one that is not in the tables.
 */
export function layoutFor(name: string | null | undefined): Layout | null {
  if (!name) return null;
  const id = name.trim().toLowerCase().replace(/_/g, '-').replace(/[()]/g, '');
  const find = (wanted: string) =>
    LAYOUTS.find((candidate) => candidate.id === wanted) ??
    LAYOUTS.find((candidate) => candidate.id === ALIASES[wanted]);
  const direct = find(id);
  if (direct) return direct;
  const parts = id.split('-');
  // `de-nodeadkeys` is `de`; a language tag's country says more than its language (`de-CH`).
  return (parts[1] ? find(parts[1]) : null) ?? find(parts[0]!) ?? null;
}

/** A guess from the languages the system prefers, when nothing better is known. */
export function layoutForLanguages(languages: readonly string[]): Layout {
  for (const tag of languages) {
    const lower = tag.toLowerCase();
    const found = (ALIASES[lower] ? layoutFor(lower) : null) ?? layoutFor(lower.split('-')[0]!);
    // "en" alone says nothing about the keyboard; a later language might.
    if (found && !(found.id === 'us' && lower === 'en')) return found;
  }
  return LAYOUTS[0]!;
}

/** What each key of a layout has printed on it, by code. */
export function printedKeys(of: Layout): Record<string, string> {
  const printed: Record<string, string> = { ...of.extra };
  ROW_CODES.forEach((codes, row) => {
    const characters = [...of.rows[row]!];
    codes.forEach((code, index) => {
      const character = characters[index];
      if (character && character !== ' ') printed[code] = character;
    });
  });
  return printed;
}

/**
 * The layout whose keys fit best what the system says is on them
 * (`navigator.keyboard.getLayoutMap()`), for the keyboard's shape.
 */
export function closestLayout(printed: Readonly<Record<string, string>>): Layout {
  let best = LAYOUTS[0]!;
  let bestScore = -1;
  for (const candidate of LAYOUTS) {
    const keys = printedKeys(candidate);
    const score = Object.entries(printed).filter(([code, key]) => keys[code] === key).length;
    if (score > bestScore) {
      best = candidate;
      bestScore = score;
    }
  }
  return best;
}

export type Key = {
  code: string;
  /** Width in key units; 1 when left out. */
  width?: number;
};

const key = (code: string, width?: number): Key => ({ code, width });

/** The keyboard's main block, row by row, in this shape. Every row is 15 units wide. */
export function mainRows(geometry: Geometry, mac: boolean): Key[][] {
  const [numbers, top, home, bottom] = ROW_CODES.map((codes) => codes.map((code) => key(code)));
  const jis = geometry === 'jis';
  const tall = geometry !== 'ansi';
  const functions = Array.from({ length: 12 }, (_, i) => key(`F${i + 1}`, 14 / 12));
  return [
    [key('Escape'), ...functions],
    [...numbers!, ...(jis ? [key('IntlYen'), key('Backspace')] : [key('Backspace', 2)])],
    [key('Tab', 1.5), ...top!, ...(tall ? [key('Enter', 1.5)] : [key('Backslash', 1.5)])],
    [
      key('CapsLock', 1.75),
      ...home!.slice(0, 11),
      ...(tall ? [key('Backslash'), key('Enter', 1.25)] : [key('Enter', 2.25)]),
    ],
    [
      key('ShiftLeft', tall && !jis ? 1.25 : 2.25),
      ...(tall && !jis ? [bottom![0]!] : []),
      ...bottom!.slice(1),
      ...(jis || geometry === 'abnt'
        ? [key('IntlRo'), key('ShiftRight', 1.75)]
        : [key('ShiftRight', 2.75)]),
    ],
    mac
      ? [
          key('ControlLeft', 1.5),
          key('AltLeft', 1.5),
          key('MetaLeft', 1.75),
          key('Space', 5.5),
          key('MetaRight', 1.75),
          key('AltRight', 1.5),
          key('ControlRight', 1.5),
        ]
      : [
          key('ControlLeft', 1.5),
          key('MetaLeft', 1.25),
          key('AltLeft', 1.5),
          key('Space', 6),
          key('AltRight', 1.5),
          key('MetaRight', 1.25),
          key('ControlRight', 2),
        ],
  ];
}

/** The block right of the main one: Insert to Page Down, and the arrows. */
export const NAVIGATION_ROWS: readonly (readonly (Key | null)[])[] = [
  [key('Insert'), key('Home'), key('PageUp')],
  [key('Delete'), key('End'), key('PageDown')],
  [null, null, null],
  [null, key('ArrowUp'), null],
  [key('ArrowLeft'), key('ArrowDown'), key('ArrowRight')],
];
