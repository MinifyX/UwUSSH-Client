import assert from 'node:assert/strict';
import { test } from 'node:test';
import { stepTab } from '../src/lib/tabRound.ts';

test('next and previous tab go round, with the overview as the first stop', () => {
  const ids = ['a', 'b'];
  assert.equal(stepTab(ids, null, 1), 'a');
  assert.equal(stepTab(ids, 'b', 1), null);
  assert.equal(stepTab(ids, null, -1), 'b');
  assert.equal(stepTab(ids, 'a', -1), null);
  assert.equal(stepTab(ids, 'gone', 1), 'a');
});
