// Exercise the shipped adapter without a browser framework or client backend.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
const source = await readFile(process.argv[2]);
const { bindPanelControls } = await import(`data:text/javascript;base64,${source.toString('base64')}`);
const requests = [];
const buttons = ['generate-one', 'generate-selected', 'review', 'undeclared'].map(action => ({
  dataset: { action, panelId: 'p1' }, disabled: false, listener: null,
  addEventListener(_name, callback) { this.listener = callback; },
  removeEventListener(_name, callback) { assert.equal(this.listener, callback); this.listener = null; }
}));
const status = { textContent: '' };
const root = { querySelectorAll: () => buttons, querySelector: () => status };
bindPanelControls(root, undefined, () => []);
assert(buttons.every(button => button.disabled && !button.listener));
buttons.forEach(button => { button.disabled = false; });
let release;
const detach = bindPanelControls(root, {
  submit(request) { requests.push(request); return new Promise(resolve => { release = resolve; }); }
}, () => ['p1', 'p2']);
buttons[3].listener();
assert.equal(requests.length, 0);
for (let index = 0; index < 3; index++) {
  buttons[index].listener();
  buttons[index].listener();
  assert.equal(requests.length, index + 1, 'duplicate clicks must not submit twice');
  assert(buttons.every(button => button.disabled));
  release({ outcome: 'succeeded' });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(status.textContent, 'Completed');
}
assert.deepEqual(requests.map(request => request.inputs.panel_ids), [['p1'], ['p1', 'p2'], ['p1', 'p2']]);
detach();
assert(buttons.every(button => !button.listener));
process.stdout.write(JSON.stringify(requests));
