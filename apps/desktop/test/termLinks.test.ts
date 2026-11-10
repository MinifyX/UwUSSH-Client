import assert from 'node:assert/strict';
import { test } from 'node:test';
import { osc52Text, wrappedLinks } from '../src/lib/termLinks.ts';

test('OSC 52 writes are decoded, reads and junk are not', () => {
  assert.equal(osc52Text(`c;${btoa('https://example.com/a')}`), 'https://example.com/a');
  assert.equal(osc52Text(`;${Buffer.from('grüß', 'utf8').toString('base64')}`), 'grüß');
  assert.equal(osc52Text('c;?'), null);
  assert.equal(osc52Text('c;'), null);
  assert.equal(osc52Text('c;not base64!'), null);
  assert.equal(osc52Text('nosemicolon'), null);
});

test('an address broken over lines at the right edge is one link', () => {
  const cols = 30;
  const lines = [
    'Open:',
    '  https://example.com/a?x=1&y=',
    '  2&z=3&scope=read+write+more',
    '  tail',
    '',
    'done',
  ];
  assert.deepEqual(wrappedLinks(lines, cols), [
    {
      url: 'https://example.com/a?x=1&y=2&z=3&scope=read+write+moretail',
      firstLine: 1,
      lastLine: 3,
      start: 2,
      end: 6,
    },
  ]);
});

test('short lines, a second address and text after the address end the link', () => {
  const cols = 30;
  assert.deepEqual(wrappedLinks(['  https://example.com/x', 'next'], cols), []);
  assert.deepEqual(
    wrappedLinks(['see https://example.org/abcdefghij', 'https://example.net/'], cols),
    [],
  );
  const [link] = wrappedLinks(
    ['go https://example.org/abcdefghij', 'klm and more text here'],
    cols,
  );
  assert.equal(link?.url, 'https://example.org/abcdefghijklm');
  assert.equal(link?.end, 3);
});
