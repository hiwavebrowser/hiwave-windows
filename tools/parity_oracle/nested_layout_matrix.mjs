// Nested layout matrix: one layout mode inside another, against Chromium.
//
// Each case is a tiny page: an outer container of one mode holding an inner
// container of another, holding a few fixed-size boxes. Every element has an
// id; Chromium reports the border box of each (getBoundingClientRect, not
// rounded) and the engine test (crates/rustkit-engine,
// nested_layout_matrix_tests) lays the same page out and compares x, y,
// width and height within 1px.
//
// No case has text: the page sets `font-size: 0; line-height: 0` and every
// leaf is an empty box with a fixed size (or a size that comes only from
// stretch, percentages or the container), so font metrics never decide a
// case. With no strut a baseline shows nowhere on its own, hence the `#s`
// probe beside the inline-block outer (see OUTER).
//
//   node nested_layout_matrix.mjs              list the generated cases
//   node nested_layout_matrix.mjs --write      capture in Chromium and write
//                                              nested_layout_matrix_cases.json
//
// The capture uses Playwright's Chromium, or PARITY_CHROME_PATH if set (see
// deterministic.mjs). The version is stored in the file as `chrome`.
import { writeFileSync } from 'node:fs';

const FILE = new URL('./nested_layout_matrix_cases.json', import.meta.url);
const VIEWPORT = [1280, 800];

// ── outer modes ────────────────────────────────────────────────────────────
// Each wraps the inner markup. Every outer box is 300px tall so a percentage
// height inside has something to resolve against; the shrink-to-fit ones
// (float, inline-block, absolute) have no width.
const OUTER = {
  block: (i) => `<div id="o" style="width:600px;height:300px">${i}</div>`,
  float: (i) => `<div id="o" style="float:left;height:300px">${i}</div>`,
  // A 10x10 inline-block `#s` shares the line, baseline-aligned: with no
  // strut (font-size 0) its y is where the outer box's baseline is, which
  // the outer box takes from its content (a flex or grid container's first
  // item, a block's last line, else its bottom edge). Without it the
  // baseline would decide nothing here and only show with a real font.
  'inline-block': (i) =>
    `<div id="o" style="display:inline-block;height:300px">${i}</div>` +
    `<div id="s" style="display:inline-block;width:10px;height:10px"></div>`,
  'flex-row': (i) => `<div id="o" style="display:flex;width:600px;height:300px">${i}</div>`,
  'flex-col': (i) =>
    `<div id="o" style="display:flex;flex-direction:column;width:600px;height:300px">${i}</div>`,
  grid: (i) => `<div id="o" style="display:grid;width:600px;height:300px">${i}</div>`,
  abspos: (i) =>
    `<div id="q" style="position:relative;width:600px;height:300px">` +
    `<div id="o" style="position:absolute;left:10px;top:10px;height:300px">${i}</div></div>`,
  'table-cell': (i) =>
    `<div id="t" style="display:table;width:600px;height:300px">` +
    `<div id="o" style="display:table-cell">${i}</div></div>`,
};
const ALL = Object.keys(OUTER);

// ── inner modes ────────────────────────────────────────────────────────────
const INNER = {
  block: 'display:block',
  'flex-row': 'display:flex',
  'flex-col': 'display:flex;flex-direction:column',
  grid: 'display:grid;grid-template-columns:auto auto',
  'inline-block': 'display:inline-block',
};

// The three children of the core cases: two fixed boxes, and one with only a
// height (its width comes from the container: full width in a block, 0 in a
// flex row, stretched in a flex column or a grid cell).
const A = '<div id="a" style="width:50px;height:20px"></div>';
const B = '<div id="b" style="width:70px;height:30px"></div>';
const C = '<div id="c" style="height:10px"></div>';
const box = (id, css) => `<div id="${id}" style="${css}"></div>`;

const WIDTHS = ['auto', '50%', '200px', 'intrinsic'];
const HEIGHTS = ['auto', '50%', '120px'];
// min-content / max-content / fit-content rotate with the height, so each
// keyword meets every outer and inner mode without tripling the matrix.
const INTRINSIC = { auto: 'min-content', '50%': 'max-content', '120px': 'fit-content' };
const slug = (v) => v.replace('%', 'pct');

function page(body) {
  return (
    '<!doctype html><html><head><style>body{margin:0;font-size:0;line-height:0}</style></head>' +
    `<body><div id="r" style="padding:4px 8px">${body}</div></body></html>`
  );
}

const cases = [];
function add(name, group, outer, inner) {
  cases.push({ name: `${group}--${outer}--${name}`, group, html: page(OUTER[outer](inner)) });
}

// ── core: outer x inner x width x height ───────────────────────────────────
for (const outer of ALL) {
  for (const [im, display] of Object.entries(INNER)) {
    for (const h of HEIGHTS) {
      for (const w0 of WIDTHS) {
        const w = w0 === 'intrinsic' ? INTRINSIC[h] : w0;
        const css = [display, w !== 'auto' && `width:${w}`, h !== 'auto' && `height:${h}`]
          .filter(Boolean)
          .join(';');
        cases.push({
          name: `core--${outer}--${im}--w-${slug(w)}--h-${slug(h)}`,
          group: 'core',
          html: page(OUTER[outer](`<div id="i" style="${css}">${A}${B}${C}</div>`)),
        });
      }
    }
  }
}

// ── riders: each crossed with the outer modes where it means something ─────
const KEY4 = ['block', 'flex-row', 'flex-col', 'grid'];
const riders = [
  // aspect-ratio on the inner box: alone, with a fixed width, with a fixed
  // height; and a ratio item in a grid cell (stretch is not applied to it).
  ['aspect-ratio', 'auto', KEY4, '<div id="i" style="aspect-ratio:2/1"></div>'],
  ['aspect-ratio', 'fixed-width', KEY4, '<div id="i" style="width:100px;aspect-ratio:2/1"></div>'],
  ['aspect-ratio', 'fixed-height', KEY4, '<div id="i" style="height:60px;aspect-ratio:2/1"></div>'],
  [
    'aspect-ratio',
    'in-a-grid-cell',
    KEY4,
    `<div id="i" style="display:grid;grid-template-columns:100px 200px">` +
      `${box('a', 'aspect-ratio:1/1')}${box('b', 'height:30px')}</div>`,
  ],
  // A percentage height two levels down, through an auto-height box and
  // through a definite one.
  [
    'percent-height',
    'through-auto',
    [...KEY4, 'abspos', 'table-cell'],
    `<div id="m"><div id="i" style="width:100px;height:50%">${box('a', 'width:40px;height:50%')}</div></div>`,
  ],
  [
    'percent-height',
    'through-definite',
    [...KEY4, 'abspos', 'table-cell'],
    `<div id="m" style="height:200px"><div id="i" style="width:100px;height:50%">` +
      `${box('a', 'width:40px;height:50%')}</div></div>`,
  ],
  // Grid spans: a column span and a row span, with and without
  // grid-auto-rows.
  [
    'grid-span',
    'auto-rows-40',
    ALL,
    `<div id="i" style="display:grid;grid-template-columns:repeat(3,50px);grid-auto-rows:40px">` +
      `${box('a', 'grid-column:span 2')}${box('b', 'grid-row:span 2')}${box('c', '')}${box('d', '')}</div>`,
  ],
  [
    'grid-span',
    'auto-rows-auto',
    KEY4,
    `<div id="i" style="display:grid;grid-template-columns:repeat(3,50px)">` +
      `${box('a', 'grid-column:1/span 2;height:20px')}${box('b', 'grid-row:span 2;height:70px')}` +
      `${box('c', 'height:10px')}${box('d', 'height:15px')}</div>`,
  ],
  // min/max clamps on the inner container.
  ['clamp', 'min-width-flex', ['block', 'flex-row', 'grid'], `<div id="i" style="display:flex;min-width:300px">${A}${B}${C}</div>`],
  ['clamp', 'max-width-flex', ['block', 'flex-row', 'grid'], `<div id="i" style="display:flex;max-width:100px">${A}${B}${C}</div>`],
  ['clamp', 'min-height-flex', ['block', 'flex-row', 'grid'], `<div id="i" style="display:flex;min-height:100px">${A}${B}${C}</div>`],
  [
    'clamp',
    'max-height-grid',
    ['block', 'flex-row', 'grid'],
    `<div id="i" style="display:grid;grid-template-columns:auto auto;max-height:20px">${A}${B}${C}</div>`,
  ],
  // An absolutely positioned child of the inner container, sized by its
  // insets.
  [
    'abspos-child',
    'inset-in-flex',
    KEY4,
    `<div id="i" style="display:flex;position:relative;height:100px">${A}${box('p', 'position:absolute;inset:10px')}</div>`,
  ],
  [
    'abspos-child',
    'top-bottom-in-grid',
    KEY4,
    `<div id="i" style="display:grid;position:relative;grid-template-columns:100px 100px">${A}${B}` +
      `${box('p', 'position:absolute;top:5px;bottom:5px;left:0;width:30px')}</div>`,
  ],
  [
    'abspos-child',
    'left-right-in-block',
    KEY4,
    `<div id="i" style="position:relative;height:80px">${A}` +
      `${box('p', 'position:absolute;left:10px;right:20px;top:0;height:10px')}</div>`,
  ],
  // margin: auto on an item.
  [
    'margin-auto',
    'flex',
    KEY4,
    `<div id="i" style="display:flex;height:100px">${box('a', 'width:50px;height:20px;margin:auto')}${B}</div>`,
  ],
  [
    'margin-auto',
    'grid',
    KEY4,
    `<div id="i" style="display:grid;grid-template-columns:200px;grid-template-rows:100px">` +
      `${box('a', 'width:50px;height:20px;margin:auto')}</div>`,
  ],
  // gap.
  [
    'gap',
    'flex-wrap',
    KEY4,
    `<div id="i" style="display:flex;flex-wrap:wrap;gap:10px 20px;width:150px">${A}${B}${box('d', 'width:50px;height:20px')}</div>`,
  ],
  [
    'gap',
    'grid',
    KEY4,
    `<div id="i" style="display:grid;grid-template-columns:50px 50px;gap:10px 20px">` +
      `${box('a', 'height:20px')}${box('b', 'height:20px')}${box('c', 'height:20px')}${box('d', 'height:20px')}</div>`,
  ],
  // box-sizing: border-box with padding and border, on the container and an
  // item.
  [
    'border-box',
    'flex',
    KEY4,
    `<div id="i" style="display:flex;box-sizing:border-box;width:200px;height:100px;padding:10px;border:5px solid #000">` +
      `${A}${box('b', 'box-sizing:border-box;width:70px;height:30px;padding:5px;border:3px solid #000')}</div>`,
  ],
  [
    'border-box',
    'grid',
    KEY4,
    `<div id="i" style="display:grid;grid-template-columns:1fr 1fr;box-sizing:border-box;width:50%;height:100px;` +
      `padding:10px 20px;border:4px solid #000">${A}${B}</div>`,
  ],
  // A wrapping flex row, auto width (shrink-to-fit outers decide the line
  // length) and fixed width.
  [
    'flex-wrap',
    'auto-width',
    ALL,
    `<div id="i" style="display:flex;flex-wrap:wrap">` +
      `${box('a', 'width:100px;height:20px')}${box('b', 'width:100px;height:20px')}` +
      `${box('c', 'width:100px;height:20px')}${box('d', 'width:100px;height:20px')}</div>`,
  ],
  [
    'flex-wrap',
    'fixed-width',
    KEY4,
    `<div id="i" style="display:flex;flex-wrap:wrap;width:120px">` +
      `${box('a', 'width:50px;height:20px')}${box('b', 'width:50px;height:30px')}` +
      `${box('c', 'width:50px;height:20px')}${box('d', 'width:50px;height:20px')}</div>`,
  ],
  // align-items / justify-content: stretch against center.
  [
    'align',
    'flex-center',
    ['block', 'flex-col', 'grid'],
    `<div id="i" style="display:flex;width:300px;height:100px;align-items:center;justify-content:center">${A}${B}${C}</div>`,
  ],
  [
    'align',
    'flex-stretch',
    ['block', 'flex-col', 'grid'],
    `<div id="i" style="display:flex;width:300px;height:100px">${A}${box('b', 'width:70px')}${C}</div>`,
  ],
  [
    'align',
    'grid-center',
    ['block', 'flex-col', 'grid'],
    `<div id="i" style="display:grid;grid-template-columns:1fr 1fr;width:300px;height:100px;place-items:center">${A}${B}</div>`,
  ],
  [
    'align',
    'grid-stretch',
    ['block', 'flex-col', 'grid'],
    `<div id="i" style="display:grid;grid-template-columns:1fr 1fr;width:300px;height:100px">${box('a', '')}${B}</div>`,
  ],
  // An empty block as a flex item (its automatic minimum width is 0), and an
  // item whose content keeps it from shrinking.
  [
    'flex-min-auto',
    'empty-basis-0',
    KEY4,
    `<div id="i" style="display:flex;width:100px">${box('a', 'flex:1 1 0;height:20px')}${box('b', 'width:200px;height:20px')}</div>`,
  ],
  [
    'flex-min-auto',
    'content-overflows',
    KEY4,
    `<div id="i" style="display:flex;width:100px"><div id="a" style="height:20px">${box('d', 'width:150px;height:10px')}</div>${B}</div>`,
  ],
  // A flex container in a shrink-to-fit box.
  [
    'shrink-to-fit-flex',
    'grow',
    ['float', 'inline-block', 'abspos'],
    `<div id="i" style="display:flex">${box('a', 'flex:1;width:50px;height:20px')}${B}</div>`,
  ],
  [
    'shrink-to-fit-flex',
    'wrap',
    ['float', 'inline-block', 'abspos'],
    `<div id="i" style="display:flex;flex-wrap:wrap">${A}${B}</div>`,
  ],
  [
    'shrink-to-fit-flex',
    'column',
    ['float', 'inline-block', 'abspos'],
    `<div id="i" style="display:flex;flex-direction:column">${A}${B}${C}</div>`,
  ],
];
for (const [group, name, outers, inner] of riders) {
  for (const outer of outers) add(name, group, outer, inner);
}

const names = new Set();
for (const c of cases) {
  if (names.has(c.name)) throw new Error('duplicate case ' + c.name);
  names.add(c.name);
}

if (!process.argv.includes('--write')) {
  for (const c of cases) console.log(c.name);
  console.log(cases.length + ' cases');
  process.exit(0);
}

const { chromium } = await import('playwright');
const { getDeterministicLaunchOptions } = await import('./deterministic.mjs');
const RECTS = `Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {
  var r = e.getBoundingClientRect();
  return [e.id, r.left, r.top, r.width, r.height];
})`;
const round = (v) => Math.round(v * 100) / 100;
const browser = await chromium.launch(getDeterministicLaunchOptions());
const version = browser.version();
const context = await browser.newContext({ viewport: { width: VIEWPORT[0], height: VIEWPORT[1] }, deviceScaleFactor: 1 });
const page_ = await context.newPage();
for (const c of cases) {
  await page_.setContent(c.html);
  c.rects = (await page_.evaluate(RECTS)).map(([id, ...r]) => [id, ...r.map(round)]);
}
await browser.close();
// One case per line, so the file diffs (and reviews) by case.
const head = {
  chrome: version,
  viewport: VIEWPORT,
  device_scale_factor: 1,
  generator: 'tools/parity_oracle/nested_layout_matrix.mjs',
  rects: 'border box per element with an id: [id, x, y, width, height], getBoundingClientRect, 2 decimals',
};
const lines = Object.entries(head).map(([k, v]) => ` ${JSON.stringify(k)}: ${JSON.stringify(v)},`);
writeFileSync(
  FILE,
  ['{', ...lines, ' "cases": [', cases.map((c) => '  ' + JSON.stringify(c)).join(',\n'), ' ]', '}', ''].join('\n'),
);
console.log(`${cases.length} cases written, chrome ${version}`);
