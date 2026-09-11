// Offline research candidates, never an automatic asset promotion pipeline.
const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');
const crypto = require('node:crypto');
const assert = require('node:assert/strict');
const svgoDir = process.argv[2];
const out = process.argv[3];
if (!svgoDir || !out) throw Error('usage: node generate.cjs SVGO_PACKAGE_DIR OUTPUT_DIR');
const { optimize } = require(path.resolve(svgoDir));
const { path2js } = require(path.resolve(svgoDir, 'plugins/_path.js'));
const version = require(path.resolve(svgoDir, 'package.json')).version;
assert.equal(version, '3.3.2', 'the evaluation pins SVGO');
const baselineConfig = {
  multipass: false,
  floatPrecision: 2,
  plugins: [{ name: 'preset-default', params: { overrides: { removeViewBox: false } } }],
};
const hash = s => crypto.createHash('sha256').update(s).digest('hex');
const sizes = s => ({ raw: Buffer.byteLength(s), gzip: zlib.gzipSync(s, { level: 9, mtime: 0 }).length,
  brotli: zlib.brotliCompressSync(s, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11,
    [zlib.constants.BROTLI_PARAM_MODE]: zlib.constants.BROTLI_MODE_TEXT } }).length });
// Factor only fill, not opacity: group opacity changes overlapping-path compositing.
const palette = { name: 'factor-fill-classes', fn: () => ({ element: { enter(node) {
  if (node.name !== 'svg') return;
  const counts = new Map();
  const visit = (n, fn) => { if (n.type === 'element') fn(n); (n.children || []).forEach(c => visit(c, fn)); };
  visit(node, n => { if (n.name === 'path' && n.attributes.fill && !n.attributes.class)
    counts.set(n.attributes.fill, (counts.get(n.attributes.fill) || 0) + 1); });
  const colors = [...counts].filter(([c, n]) => n >= 2 && /^#[0-9a-f]+$/i.test(c)).map(([c]) => c).sort();
  if (!colors.length) return;
  const classes = new Map(colors.map((c, i) => [c, `p${i}`]));
  visit(node, n => { if (n.name === 'path' && !n.attributes.class && classes.has(n.attributes.fill)) {
    n.attributes.class = classes.get(n.attributes.fill); delete n.attributes.fill;
  }});
  node.children.unshift({ type: 'element', name: 'style', attributes: {}, children: [
    { type: 'text', value: colors.map(c => `.${classes.get(c)}{fill:${c}}`).join('') }] });
}}}) };
const structural = ['removeEmptyAttrs', 'removeEmptyContainers', 'collapseGroups', 'sortAttrs'];
const numeric = ['cleanupNumericValues', 'convertPathData', 'convertTransform'];
const consolidation = ['moveElemsAttrsToGroup', 'collapseGroups', 'mergePaths'];
// Conservative control-point hull. Unknown commands/transforms/strokes are ineligible.
// 37.5 = largest tested scale (4800 / 128), deliberately overestimates screen scale.
const subpixel = { name: 'trial-subpixel-hulls', fn: () => ({ element: { enter(node) {
  if (node.name !== 'svg' || node.attributes.transform || node.attributes.viewBox || node.attributes.stroke) return;
  node.children = node.children.filter(n => {
    if (n.name !== 'path' || Object.keys(n.attributes).some(k => !['d', 'fill', 'fill-opacity'].includes(k))) return true;
    const commands = path2js(n);
    if (commands.some(c => !['M', 'L', 'C', 'Q', 'Z'].includes(c.command.toUpperCase()))) return true;
    const xs = [], ys = [];
    let x = 0, y = 0, startX = 0, startY = 0;
    for (const c of commands) {
      if (c.command.toUpperCase() === 'Z') { x = startX; y = startY; continue; }
      const dx = c.command === c.command.toLowerCase() ? x : 0;
      const dy = c.command === c.command.toLowerCase() ? y : 0;
      for (let i = 0; i < c.args.length; i += 2) { xs.push(c.args[i] + dx); ys.push(c.args[i + 1] + dy); }
      x = xs.at(-1); y = ys.at(-1);
      if (c.command.toUpperCase() === 'M') { startX = x; startY = y; }
    }
    const extent = Math.max(Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys));
    return !(extent * 37.5 < 0.25);
  });
}}}) };
const candidates = {
  structural, numeric, palette: [palette], reuse: ['reusePaths'], consolidation,
  structural_numeric: [...structural, ...numeric],
  palette_reuse: [palette, 'reusePaths'],
  subpixel: [subpixel],
  // Deliberately aggressive control: must fail the same exact rendered-output gate.
  precision1: [{ name: 'convertPathData', params: { floatPrecision: 1 } }],
};
const manifest = { schema: 1, svgo: version, node: process.version, zlib: process.versions.zlib,
  brotli: process.versions.brotli, baselineConfig, assets: {} };
fs.mkdirSync(out, { recursive: true });
for (const asset of ['character', 'screen']) {
  const master = fs.readFileSync(path.join(__dirname, 'corpus', `${asset}.svg`));
  const expected = asset === 'character' ? '7a43e3fe96a45e3a53f199d0f12904b36bcc82f332ab780a89646328f86e31cc' :
    '5c7dcf1b4a0292f5b8057130b116f70498c586f6bbda26332f5417ae58fc7528';
  assert.equal(hash(master), expected, 'immutable corpus changed');
  const baseline = optimize(master.toString(), baselineConfig).data;
  assert.equal(baseline, optimize(master.toString(), baselineConfig).data);
  const dir = path.join(out, asset); fs.mkdirSync(dir, { recursive: true });
  const records = { master: { sha256: hash(master), ...sizes(master) } };
  for (const [name, plugins] of Object.entries({ baseline: null, ...candidates })) {
    const run = () => plugins ? optimize(baseline, { floatPrecision: name === 'precision1' ? 1 : 2, multipass: false, plugins }).data : baseline;
    const data = run(); assert.equal(data, run(), `${asset}/${name} unstable`);
    fs.writeFileSync(path.join(dir, `${name}.svg`), data);
    records[name] = { sha256: hash(data), ...sizes(data), identicalBaseline: data === baseline };
  }
  manifest.assets[asset] = records;
}
fs.writeFileSync(path.join(out, 'sizes.json'), JSON.stringify(manifest, null, 2) + '\n');
