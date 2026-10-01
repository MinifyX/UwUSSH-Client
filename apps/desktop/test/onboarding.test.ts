// When the first-start setup shows. Runs with `node --test` (Node strips the types).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { onboardingDecision, type InstallSigns } from '../src/lib/onboarding.ts';

const FRESH: InstallSigns = {
  done: false,
  settingsStored: false,
  hosts: 0,
  vault: 'absent',
  sync: 'none',
};

test('a fresh install gets the setup', () => {
  assert.equal(onboardingDecision(FRESH), 'show');
});

test('once it ran it never shows by itself again', () => {
  assert.equal(onboardingDecision({ ...FRESH, done: true }), 'skip');
  assert.equal(onboardingDecision({ ...FRESH, done: true, hosts: 3 }), 'skip');
});

test('anyone who used the app before is left alone, and that is remembered', () => {
  assert.equal(onboardingDecision({ ...FRESH, hosts: 1 }), 'settled');
  assert.equal(onboardingDecision({ ...FRESH, vault: 'locked' }), 'settled');
  assert.equal(onboardingDecision({ ...FRESH, vault: 'unlocked' }), 'settled');
  assert.equal(onboardingDecision({ ...FRESH, sync: 'uwusync' }), 'settled');
  assert.equal(onboardingDecision({ ...FRESH, sync: 'uwulock' }), 'settled');
  assert.equal(onboardingDecision({ ...FRESH, settingsStored: true }), 'settled');
});

test('one clear sign is enough even when the rest could not be read', () => {
  assert.equal(onboardingDecision({ ...FRESH, hosts: null, vault: 'locked' }), 'settled');
  assert.equal(onboardingDecision({ ...FRESH, hosts: 2, vault: null, sync: null }), 'settled');
});

test('when the app cannot tell, it does not show the setup and does not decide', () => {
  assert.equal(onboardingDecision({ ...FRESH, hosts: null }), 'skip');
  assert.equal(onboardingDecision({ ...FRESH, vault: null }), 'skip');
});

test('an unreadable sync state alone does not stop a fresh install', () => {
  assert.equal(onboardingDecision({ ...FRESH, sync: null }), 'show');
});
