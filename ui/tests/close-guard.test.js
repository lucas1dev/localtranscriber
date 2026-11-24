import test from 'node:test';
import assert from 'node:assert/strict';
import { CLOSE_CONFIRM, CLOSE_EVENT } from '../js/views/close-guard.js';
import { api } from '../js/api.js';

test('confirmação de fechar durante a gravação', () => {
  assert.equal(CLOSE_EVENT, 'app://close-requested');
  assert.equal(CLOSE_CONFIRM, 'Uma gravação está em andamento. Fechar agora descarta a gravação. Fechar mesmo assim?');
  assert.equal(typeof api.confirmClose, 'function');
});
