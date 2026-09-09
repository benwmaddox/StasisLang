import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';

test('both modes must be saved before the player and rating legends reveal bitrates', async () => {
  const source = (await readFile(new URL('./app.js', import.meta.url), 'utf8'))
    .replace("'./report.mjs'", JSON.stringify(new URL('./report.mjs', import.meta.url).href));
  const originals = Object.fromEntries(['document', 'localStorage', 'fetch', 'FormData'].map(key => [key, globalThis[key]]));
  try {
    for (const firstMode of ['isolated', 'gameplay']) {
      const element = () => ({
        children: [], value: '', textContent: '', innerHTML: '',
        append(child) { this.children.push(child); },
        replaceChildren() { this.children = []; },
        querySelector() { return {}; },
        pause() {}, load() {}, removeAttribute() {},
      });
      const ids = Object.fromEntries(['player', 'track', 'mode', 'versions', 'fields',
        'status', 'report', 'recommendation', 'ratings', 'restart', 'export'].map(id => [id, element()]));
      ids.track.value = '0'; ids.mode.value = firstMode;
      const storage = new Map();
      globalThis.document = {getElementById: id => ids[id], createElement: element};
      globalThis.localStorage = {getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value)};
      globalThis.fetch = async () => ({ok: true, json: async () => ({experimentId: 'test', tracks: [{
        id: 'cozy', name: 'Cozy', reference: 'reference.wav',
        variants: [32, 24, 20, 16].map(bitrateKbps => ({bitrateKbps, path: `${bitrateKbps}.opus`, sizeBytes: 1000, durationSeconds: 1})),
      }]})});
      globalThis.FormData = class { get(name) { return name.startsWith('score') ? '4' : name.startsWith('acceptable') ? 'yes' : ''; } };
      await import(`data:text/javascript;base64,${Buffer.from(source + `\n// ${firstMode}`).toString('base64')}`);
      const assertHidden = () => {
        assert.equal(ids.versions.children.some(button => button.textContent.includes('kbps')), false);
        assert.equal(ids.fields.children.some(field => /<legend>.*kbps/.test(field.innerHTML)), false);
      };
      assertHidden();
      ids.ratings.onsubmit({preventDefault() {}});
      assertHidden();
      assert.match(ids.status.textContent, /both listening modes/);
      ids.mode.value = firstMode === 'isolated' ? 'gameplay' : 'isolated';
      ids.mode.onchange();
      assertHidden();
      ids.ratings.onsubmit({preventDefault() {}});
      assert.equal(ids.versions.children.filter(button => button.textContent.includes('kbps')).length, 4);
      assert.equal(ids.fields.children.filter(field => /<legend>.*kbps/.test(field.innerHTML)).length, 4);
      ids.mode.value = firstMode; ids.mode.onchange();
      assert.equal(ids.versions.children.filter(button => button.textContent.includes('kbps')).length, 4);
    }
  } finally {
    for (const [key, value] of Object.entries(originals)) {
      if (value === undefined) delete globalThis[key]; else globalThis[key] = value;
    }
  }
});
