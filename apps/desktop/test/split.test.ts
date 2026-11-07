// Two tabs side by side (lib/split.ts). Runs with `node --test`.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { pruneSplit, splitShown, splitWith, toggleSplit } from '../src/lib/split.ts';

test('a split needs two different tabs', () => {
  assert.equal(splitWith(null, 'b'), null);
  assert.equal(splitWith('a', 'a'), null);
  assert.deepEqual(splitWith('a', 'b', true), { a: 'a', b: 'b', vertical: true });
});

test('a split shows while one of its tabs is in front', () => {
  const split = splitWith('a', 'b')!;
  assert.equal(splitShown(split, 'a'), true);
  assert.equal(splitShown(split, 'b'), true);
  assert.equal(splitShown(split, 'c'), false);
  assert.equal(splitShown(split, null), false);
  assert.equal(splitShown(null, 'a'), false);
});

test('closing one of its tabs ends the split, other tabs leave it alone', () => {
  const split = splitWith('a', 'b')!;
  assert.equal(pruneSplit(split, ['a', 'b', 'c']), split);
  assert.equal(pruneSplit(split, ['b', 'c']), null);
  assert.equal(pruneSplit(split, ['a']), null);
  assert.equal(pruneSplit(null, ['a']), null);
});

test('the shortcut splits with the neighbour and ends a shown split', () => {
  const ids = ['a', 'b', 'c'];
  assert.deepEqual(toggleSplit(null, 'a', ids), { a: 'a', b: 'b', vertical: false });
  assert.deepEqual(toggleSplit(null, 'c', ids), { a: 'c', b: 'b', vertical: false });
  assert.equal(toggleSplit({ a: 'a', b: 'b', vertical: false }, 'b', ids), null);
  // A hidden split is replaced and keeps its direction.
  assert.deepEqual(toggleSplit({ a: 'a', b: 'b', vertical: true }, 'c', ids), {
    a: 'c',
    b: 'b',
    vertical: true,
  });
  assert.equal(toggleSplit(null, 'a', ['a']), null);
  assert.equal(toggleSplit(null, null, ids), null);
});
