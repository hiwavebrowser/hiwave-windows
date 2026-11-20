// inspector.js — board state for inspector.html (CH3.A1): a sample DOM tree,
// a selected element with its box model and computed styles, and console
// lines, through the hiwaveInspector entry points the host calls.
(function () {
  var board = window.__hiwaveBoard;
  var insp = window.hiwaveInspector;

  function el(tag, path, extra, children) {
    var n = { type: 'element', tag: tag, path: path, attributes: {}, classes: [], children: children || [] };
    for (var k in (extra || {})) n[k] = extra[k];
    return n;
  }
  function text(path, content) { return { type: 'text', path: path, textContent: content, children: [] }; }

  var tree = el('html', '0', {}, [
    el('head', '0.0', {}, [el('title', '0.0.0', { textContent: 'HiWave — a quieter browser' })]),
    el('body', '0.1', { classes: ['home'] }, [
      el('header', '0.1.0', { id: 'top', classes: ['site-header'] }, [
        el('a', '0.1.0.0', { classes: ['logo'], attributes: { href: '/' }, textContent: 'HiWave' }),
        el('nav', '0.1.0.1', { attributes: { 'aria-label': 'Main' } }, [
          el('a', '0.1.0.1.0', { attributes: { href: '/docs' }, textContent: 'Docs' }),
          el('a', '0.1.0.1.1', { attributes: { href: '/blog' }, textContent: 'Blog' })
        ])
      ]),
      el('main', '0.1.1', { id: 'content' }, [
        el('h1', '0.1.1.0', { textContent: 'A quieter browser' }),
        el('p', '0.1.1.1', { classes: ['lede'] }, [text('0.1.1.1.0', 'Built in Rust, with its own engine.')]),
        el('ul', '0.1.1.2', { classes: ['features'] }, [
          el('li', '0.1.1.2.0', { textContent: 'Shield' }),
          el('li', '0.1.1.2.1', { textContent: 'Workspaces' }),
          el('li', '0.1.1.2.2', { textContent: 'The Shelf' })
        ])
      ]),
      el('footer', '0.1.2', { classes: ['site-footer'], textContent: '2026' })
    ])
  ]);

  var styles = {
    selector: 'main#content',
    rect: { x: 0, y: 64, width: 1280, height: 420 },
    boxModel: {
      marginTop: 0, marginRight: 'auto', marginBottom: 0, marginLeft: 'auto',
      borderTop: 0, borderRight: 0, borderBottom: 0, borderLeft: 0,
      paddingTop: 24, paddingRight: 32, paddingBottom: 24, paddingLeft: 32,
      width: 1216, height: 372
    },
    computedStyles: {
      display: 'block', position: 'static', width: '1216px', height: '372px',
      margin: '0px auto', padding: '24px 32px', color: 'rgb(30, 30, 30)',
      'background-color': 'rgb(255, 255, 255)', 'font-family': 'system-ui, sans-serif',
      'font-size': '16px', 'font-weight': '400', 'line-height': '24px', 'max-width': '1216px'
    }
  };

  var entries = [
    { id: 1, level: 'log', message: 'HiWave page script ready', timestamp: 1700000000000, stack: [] },
    { id: 2, level: 'info', message: 'shield: 17 trackers blocked on this page', timestamp: 1700000000200, stack: [] },
    { id: 3, level: 'warn', message: 'deprecated API: document.write', timestamp: 1700000000400, stack: [] },
    { id: 4, level: 'error', message: 'TypeError: Cannot read properties of undefined (reading "length")', timestamp: 1700000000600,
      stack: ['at render (app.js:42:13)', 'at main (app.js:9:3)'] }
  ];

  function publish() {
    insp.setDomTree(tree);
    insp.markSelected('0.1.1');
    insp.setElementStyles(styles);
  }
  board.on('inspector_refresh', publish);
  board.on('inspector_select_element', function () { insp.setElementStyles(styles); });
  publish();
  insp.addConsoleEntries(entries);
})();
