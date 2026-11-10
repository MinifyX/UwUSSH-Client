import assert from 'node:assert/strict';
import { test } from 'node:test';
import { dropIndex, moveTo, stepTab } from '../src/lib/tabRound.ts';

test('next and previous tab go round, with the overview as the first stop', () => {
  const ids = ['a', 'b'];
  assert.equal(stepTab(ids, null, 1), 'a');
  assert.equal(stepTab(ids, 'b', 1), null);
  assert.equal(stepTab(ids, null, -1), 'b');
  assert.equal(stepTab(ids, 'a', -1), null);
  assert.equal(stepTab(ids, 'gone', 1), 'a');
});

test('a tab moves to the given place, clamped to the list', () => {
  const list = [{ id: 'a' }, { id: 'b' }, { id: 'c' }];
  const ids = (l: { id: string }[]) => l.map((item) => item.id).join('');
  assert.equal(ids(moveTo(list, 'a', 2)), 'bca');
  assert.equal(ids(moveTo(list, 'c', 0)), 'cab');
  assert.equal(ids(moveTo(list, 'b', 99)), 'acb');
  assert.equal(ids(moveTo(list, 'x', 0)), 'abc');
});

test('a dragged tab lands after the others whose middle it passed', () => {
  assert.equal(dropIndex([50, 150, 250], 10), 0);
  assert.equal(dropIndex([50, 150, 250], 160), 2);
  assert.equal(dropIndex([50, 150, 250], 999), 3);
});
