// Opt-in on a UI host with Edge/Chromium. Run with node --test render.test.cjs.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {spawnSync} = require('node:child_process');
const path = require('node:path');

function run(source, theme = 'light') {
  const child = spawnSync(process.execPath,[path.join(__dirname,'render.cjs'),theme],{
    input:source,encoding:'utf8',timeout:40000,maxBuffer:8*1024*1024,
  });
  if (child.status !== 0) throw Error(child.stderr || child.error || 'renderer exited');
  const result = JSON.parse(child.stdout);
  const svg = Buffer.from(result.svg,'base64').toString();
  const png = Buffer.from(result.png,'base64');
  assert.ok(png.subarray(1,4).equals(Buffer.from('PNG')));
  assert.match(svg,/<path /);
  assert.doesNotMatch(svg,/<(?:text|tspan|foreignObject|script|style|image)\b/);
  return svg;
}
test('flowchart labels, arrows and the two YAAC backgrounds', () => {
  const source = 'flowchart LR\n A["Ação<br/>München"] -->|été| B["東京"]';
  for (const [theme,background] of [['dark','#16181d'],['light','#ffffff']]) {
    const svg = run(source,theme);
    assert.ok(svg.includes(`fill="${background}"`));
    assert.match(svg,/flowchart-A-/);
    assert.match(svg,/flowchart-B-/);
    assert.match(svg,/L_A_B/);
  }
});
test('authored frontmatter and directive themes survive host theme changes', () => {
  for (const init of ['---\nconfig:\n  theme: forest\n---', '%%{init: {"theme":"forest"}}%%']) {
    const source = init+'\nsequenceDiagram\n participant A as José\n A->>A: Révision<br/>Ação';
    assert.equal(run(source,'dark'),run(source,'light'));
  }
});
test('invalid source returns a parser error instead of an error-image export', () => {
  assert.throws(() => run('flowchart LR\n A --> ]'), /parse|syntax|unexpected|expecting|lexical/i);
});
