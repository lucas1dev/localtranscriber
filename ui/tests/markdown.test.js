import test from 'node:test';
import assert from 'node:assert/strict';
import { interceptLinks, renderMarkdown } from '../js/markdown.js';

test('renderMarkdown remove formulários e controles na sanitização', () => {
  let seen;
  globalThis.window = {
    marked: { parse: (md) => `<p>${md}</p>` },
    DOMPurify: { sanitize: (html, cfg) => { seen = cfg; return html; } },
  };
  assert.equal(renderMarkdown('oi'), '<p>oi</p>');
  assert.deepEqual(seen.FORBID_TAGS, ['form', 'input', 'button', 'textarea', 'select']);
});

function click(href, inLink = true) {
  const anchor = { getAttribute: (n) => (n === 'href' ? href : null) };
  const ev = {
    prevented: false,
    target: { closest: (sel) => (sel === 'a' && inLink ? anchor : null) },
    preventDefault() { this.prevented = true; },
  };
  return ev;
}

test('link http(s) abre no navegador e não navega a janela', () => {
  const opened = [];
  for (const href of ['https://exemplo.com/a?b=1', 'http://exemplo.com/']) {
    const ev = click(href);
    interceptLinks(ev, (u) => opened.push(u));
    assert.equal(ev.prevented, true);
  }
  assert.deepEqual(opened, ['https://exemplo.com/a?b=1', 'http://exemplo.com/']);
});

test('outros links são ignorados, mas também não navegam', () => {
  const opened = [];
  for (const href of ['javascript:alert(1)', 'file:///etc/passwd', '#ancora', 'relativo.html', 'mailto:a@b.c', null]) {
    const ev = click(href);
    interceptLinks(ev, (u) => opened.push(u));
    assert.equal(ev.prevented, true, String(href));
  }
  assert.deepEqual(opened, []);
});

test('clique fora de link não é afetado', () => {
  const ev = click('https://x', false);
  interceptLinks(ev, () => assert.fail('não deveria abrir'));
  assert.equal(ev.prevented, false);
});
