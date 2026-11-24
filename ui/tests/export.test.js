import test from 'node:test';
import assert from 'node:assert/strict';
import { toSrt } from '../js/export.js';

test('toSrt formata índice, intervalo e texto', () => assert.equal(
  toSrt([
    { start: '00:00:00,000', end: '00:00:02,500', text: 'Olá' },
    { start: '00:00:02,500', end: '00:00:04,000', text: 'mundo' },
  ]),
  '1\n00:00:00,000 --> 00:00:02,500\nOlá\n\n2\n00:00:02,500 --> 00:00:04,000\nmundo',
));

test('toSrt vazio', () => assert.equal(toSrt([]), ''));
