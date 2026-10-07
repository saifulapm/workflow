// The pure pieces of the comment layer a designed plan page gets.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { formatAnchor, parseAnchor, clampQuote, unopenedCount } = require('../assets/annotate.js');

test('an anchor rounds the tap to whole percents', () => {
	assert.equal(formatAnchor({ root: 'claim-2', n: 14, x: 40.4, y: 61.6 }), 'claim-2/14@40,62');
	assert.equal(formatAnchor({ root: 'page', n: 0, x: 0, y: 100 }), 'page/0@0,100');
});

test('an element anchor parses back to its parts', () => {
	assert.deepEqual(parseAnchor('claim-2/14@40,62'), { root: 'claim-2', n: 14, x: 40, y: 62 });
	assert.deepEqual(parseAnchor('scope/3@5,95'), { root: 'scope', n: 3, x: 5, y: 95 });
	assert.deepEqual(parseAnchor('shared/0@50,50'), { root: 'shared', n: 0, x: 50, y: 50 });
	assert.deepEqual(parseAnchor('page/120@1,2'), { root: 'page', n: 120, x: 1, y: 2 });
});

test('a decision anchor parses to its name and value', () => {
	assert.deepEqual(parseAnchor('decision-gesture@hold'), { decision: 'gesture', value: 'hold' });
});

test('junk is not an anchor', () => {
	for (const junk of ['', 'claim-2/14@40', 'claim-2/x@1,1', 'lobby/1@1,1', 'decision-gesture', 'decision-@hold', 'claim-2/14@40,62 ', null, undefined, 7]) {
		assert.equal(parseAnchor(junk), null, String(junk));
	}
});

test('a quote is collapsed and capped at 120 characters', () => {
	assert.equal(clampQuote('  A saved\n\trecipe   shows  '), 'A saved recipe shows');
	const long = 'word '.repeat(40);
	assert.equal(clampQuote(long), 'word '.repeat(24).slice(0, 120));
	assert.equal(clampQuote(long).length, 120);
	assert.equal(clampQuote(''), '');
});

test('unopened counts a decision neither seen nor sent', () => {
	const pins = [
		{ id: 'AB12CD34', anchor: 'decision-gesture@tap', text: 'Tap the part', queued: true, reply: null, at: '2026-10-08' },
		{ id: 'EF56GH78', anchor: 'claim-2/3@10,10', text: 'why', queued: true, reply: null, at: '2026-10-08' },
	];
	assert.equal(unopenedCount(['store', 'gesture', 'legacy'], { store: true }, pins), 1);
	assert.equal(unopenedCount(['store', 'gesture'], { store: true }, pins), 0);
	assert.equal(unopenedCount(['store', 'gesture'], {}, []), 2);
});
