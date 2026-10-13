// The keyboard pictures. Runs with `node --test` (Node strips the types).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  closestLayout,
  LAYOUTS,
  layoutFor,
  layoutForLanguages,
  mainRows,
  printedKeys,
  ROW_CODES,
} from '../src/lib/keyboardLayouts.ts';

test('every layout fills every key, under an id of its own', () => {
  assert.equal(new Set(LAYOUTS.map((layout) => layout.id)).size, LAYOUTS.length);
  for (const layout of LAYOUTS) {
    layout.rows.forEach((row, index) => {
      assert.equal([...row].length, ROW_CODES[index]!.length, `${layout.id}, row ${index}`);
    });
  }
});

test('every row of every keyboard is as wide as the others', () => {
  for (const geometry of ['ansi', 'iso', 'jis', 'abnt'] as const) {
    for (const mac of [false, true]) {
      for (const row of mainRows(geometry, mac)) {
        const width = row.reduce((sum, key) => sum + (key.width ?? 1), 0);
        assert.ok(Math.abs(width - 15) < 1e-9, `${geometry}: ${width}`);
      }
    }
  }
});

test('a layout is found by the names systems give it', () => {
  assert.equal(layoutFor('de')?.id, 'de');
  assert.equal(layoutFor('de_nodeadkeys')?.id, 'de');
  assert.equal(layoutFor('de-CH')?.id, 'ch');
  assert.equal(layoutFor('us-dvorak')?.id, 'us-dvorak');
  assert.equal(layoutFor('klingon'), null);
  assert.equal(layoutFor(null), null);
  assert.equal(layoutForLanguages(['en', 'de-DE']).id, 'de');
  assert.equal(layoutForLanguages(['en']).id, 'us');
  assert.equal(layoutForLanguages([]).id, 'us');
});

test('the printed keys tell the layouts apart', () => {
  const german = printedKeys(layoutFor('de')!);
  assert.equal(german.KeyY, 'z');
  assert.equal(german.KeyZ, 'y');
  assert.equal(closestLayout(german).id, 'de');
  assert.equal(closestLayout(printedKeys(layoutFor('fr')!)).id, 'fr');
});
