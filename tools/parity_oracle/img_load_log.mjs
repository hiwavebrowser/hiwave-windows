// What a browser reports for an `<img>`'s load: the `load`/`error` events
// (count, order against DOMContentLoaded and the window's `load`), and
// `complete`, `naturalWidth`, `naturalHeight`, `currentSrc` read at
// DOMContentLoaded, at the window's `load`, inside the image's own handlers
// and 7 s in (past the engine's 5 s timer horizon, which a load runs in
// virtual time). The cases of the engine's `image_load_event_tests` (E54).
//
// Everything is served from a loopback server this script starts; each
// page posts its own report back to it, so the same pages measure the
// oracle's Chrome and RustKit.
//
// usage:
//   node tools/parity_oracle/img_load_log.mjs            # Chrome (headless)
//   node tools/parity_oracle/img_load_log.mjs --rustkit <parity-capture binary>
//   node tools/parity_oracle/img_load_log.mjs --only swap,same_src
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> selects the oracle binary.
import http from 'http';
import zlib from 'zlib';
import { execFile } from 'child_process';

const argv = process.argv.slice(2);
const flag = (name) => { const i = argv.indexOf(name); return i < 0 ? null : argv[i + 1]; };
const rustkit = flag('--rustkit');
const only = flag('--only') ? flag('--only').split(',') : null;

function png(width, height) {
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type), data]);
    const out = Buffer.alloc(8 + data.length + 4);
    out.writeUInt32BE(data.length, 0);
    body.copy(out, 4);
    out.writeUInt32BE(zlib.crc32(body) >>> 0, 8 + data.length);
    return out;
  };
  const head = Buffer.alloc(13);
  head.writeUInt32BE(width, 0);
  head.writeUInt32BE(height, 4);
  head.set([8, 2, 0, 0, 0], 8);
  const rows = Buffer.alloc((width * 3 + 1) * height, 0x66);
  for (let y = 0; y < height; y++) rows[y * (width * 3 + 1)] = 0;
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', head),
    chunk('IDAT', zlib.deflateSync(rows)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

const A = png(40, 30);
const B = png(60, 20);
const C = png(80, 50);
const SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12"><rect width="24" height="12" fill="green"/></svg>';
const FILES = {
  '/a.png': ['image/png', A],
  '/a2.png': ['image/png', A],
  '/b.png': ['image/png', B],
  '/c.png': ['image/png', C],
  '/v.svg': ['image/svg+xml', Buffer.from(SVG)],
  '/zero.png': ['image/png', Buffer.alloc(0)],
  '/garbage.png': ['image/png', Buffer.from('this is not an image at all')],
};
const DATA_A = 'data:image/png;base64,' + A.toString('base64');

// The harness every page starts with. `ev(tag, img)` logs a tag and what
// the image reads right then: complete,naturalWidth,naturalHeight,currentSrc.
const HARNESS = `<script>
var L=[];
function st(i){return '{'+[i.complete,i.naturalWidth,i.naturalHeight,(i.currentSrc||'').replace(location.origin,'').slice(0,24)].join(',')+'}';}
function ev(tag,i){L.push(tag+(i?st(i):''));}
function watch(i,name){['load','error'].forEach(function(t){i.addEventListener(t,function(){ev(name+'.'+t,i);});});return i;}
function all(tag){var q=document.querySelectorAll('img[id]');for(var k=0;k<q.length;k++)ev(tag+':'+q[k].id,q[k]);if(!q.length)ev(tag);}
document.addEventListener('DOMContentLoaded',function(){all('DCL');});
window.addEventListener('load',function(){all('winload');});
setTimeout(function(){all('end');var x=new XMLHttpRequest();x.open('POST','/report?case='+CASE);x.send(L.join(' | '));},7000);
</script>`;

// name -> body. Each is one behaviour; the comment says what is asked.
const CASES = {
  // An image in the markup, heard three ways, and whether `load` bubbles.
  markup: `<img id=a src=/a.png?delay=150 onload="ev('a.attr',this)"><img id=b src=/a.png?delay=150&n=b><img id=c src=/a.png?delay=150&n=c>
    <script>document.getElementById('b').onload=function(){ev('b.prop',this)};watch(document.getElementById('c'),'c');
    document.addEventListener('load',function(e){ev('doc.capture:'+e.target.id)},true);
    document.addEventListener('load',function(e){ev('doc.bubble:'+e.target.id)});</script>`,
  // The same with no delay: whatever the race gives, the element agrees with its event.
  markup_quick: `<img id=a src=/a.png><script>watch(document.getElementById('a'),'a');ev('inline:a',document.getElementById('a'));</script>`,
  // Listeners attached only at DOMContentLoaded and at the window's load.
  late_listener: `<img id=a src=/a.png?delay=150><script>
    document.addEventListener('DOMContentLoaded',function(){watch(document.getElementById('a'),'a@dcl');});
    window.addEventListener('load',function(){watch(document.getElementById('a'),'a@winload');});</script>`,
  // An image script creates and inserts while the document loads.
  inserted: `<div id=box></div><script>var i=watch(document.createElement('img'),'a');i.id='a';i.src='/a.png?delay=150';
    ev('set:a',i);document.getElementById('box').appendChild(i);</script>`,
  // In the markup with no src; script sets it later.
  src_later: `<img id=a><script>var a=watch(document.getElementById('a'),'a');ev('nosrc:a',a);
    setTimeout(function(){a.src='/a.png?delay=100';ev('set:a',a);},50);</script>`,
  // A good image, then another good one.
  swap: `<img id=a src=/a.png><script>var a=watch(document.getElementById('a'),'a'),n=0;
    a.addEventListener('load',function(){if(n++)return;a.src='/b.png?delay=200';ev('set:a',a);setTimeout(function(){ev('mid:a',a)},80);});</script>`,
  // The same src written again after it loaded.
  same_src: `<img id=a src=/a.png><script>var a=watch(document.getElementById('a'),'a'),n=0;
    a.addEventListener('load',function(){if(n++)return;setTimeout(function(){a.src='/a.png';ev('set:a',a);},100);});</script>`,
  // The same attribute value written with setAttribute.
  same_src_attr: `<img id=a src=/a.png><script>var a=watch(document.getElementById('a'),'a'),n=0;
    a.addEventListener('load',function(){if(n++)return;setTimeout(function(){a.setAttribute('src','/a.png');ev('set:a',a);},100);});</script>`,
  // Two elements, one URL; the second arrives once the first has loaded.
  shared_url: `<img id=a src=/a2.png><div id=box></div><script>var a=watch(document.getElementById('a'),'a'),n=0;
    a.addEventListener('load',function(){if(n++)return;setTimeout(function(){var b=watch(document.createElement('img'),'b');b.id='b';
    b.src='/a2.png';ev('set:b',b);document.getElementById('box').appendChild(b);},100);});</script>`,
  // Two elements with one URL, both in the markup.
  shared_markup: `<img id=a src=/a2.png?delay=100><img id=b src=/a2.png?delay=100><script>watch(document.getElementById('a'),'a');watch(document.getElementById('b'),'b');</script>`,
  // srcset with density descriptors (the probe runs at 1x).
  srcset_x: `<img id=a src=/c.png srcset="/a.png 1x, /b.png 2x"><script>watch(document.getElementById('a'),'a');</script>`,
  // srcset with width descriptors and sizes (800px viewport).
  srcset_w: `<img id=a src=/c.png srcset="/a.png 40w, /b.png 400w" sizes="50px"><script>watch(document.getElementById('a'),'a');</script>`,
  // picture: the first source whose media matches.
  picture: `<picture><source media="(min-width: 5000px)" srcset=/c.png><source media="(min-width: 100px)" srcset=/b.png><img id=a src=/a.png></picture>
    <script>watch(document.getElementById('a'),'a');</script>`,
  // loading=lazy far below the fold: not loaded, and the window's load does not wait.
  lazy: `<div style="height:5000px"></div><img id=a loading=lazy src=/a.png width=40 height=30><script>watch(document.getElementById('a'),'a');</script>`,
  // loading=lazy in the viewport.
  lazy_visible: `<img id=a loading=lazy src=/a.png?delay=100 width=40 height=30><script>watch(document.getElementById('a'),'a');</script>`,
  // display:none still loads.
  hidden: `<img id=a src=/a.png?delay=100 style="display:none"><script>watch(document.getElementById('a'),'a');</script>`,
  // new Image() that never enters the document.
  detached: `<script>var i=new Image();i.onload=function(){ev('new.onload',i)};i.onerror=function(){ev('new.onerror',i)};i.src='/a.png?delay=100';ev('set',i);
    var j=watch(document.createElement('img'),'created');j.src='/b.png?delay=100';
    var k=watch(new Image(),'broken');k.src='/missing.png';
    window.addEventListener('load',function(){ev('winload:new',i)});setTimeout(function(){ev('end:new',i);ev('end:created',j);ev('end:broken',k);},6500);</script>`,
  // An image taken out of the document before it has loaded.
  removed: `<img id=a src=/a.png?delay=200><script>var a=watch(document.getElementById('a'),'a');a.remove();
    window.addEventListener('load',function(){ev('winload:removed',a)});setTimeout(function(){ev('end:removed',a)},6500);</script>`,
  // decode(): before the image is in, after, and on a broken one.
  decode: `<img id=a src=/a.png?delay=150><img id=x src=/missing.png><script>var a=watch(document.getElementById('a'),'a'),x=document.getElementById('x');
    ev('typeof:'+typeof a.decode);
    function d(i,name){try{i.decode().then(function(v){ev(name+'.resolved:'+v,i)},function(e){ev(name+'.rejected:'+(e&&e.name),i)});}catch(e){ev(name+'.threw:'+e.name)}}
    d(a,'early');d(x,'broken');a.addEventListener('load',function(){d(a,'inload');setTimeout(function(){d(a,'late')},100);});
    var n=new Image();n.src='/b.png?delay=100';d(n,'new');</script>`,
  // An SVG image with a width and height.
  svg: `<img id=a src=/v.svg?delay=100><script>watch(document.getElementById('a'),'a');</script>`,
  // A data: URL.
  data_url: `<img id=a src="${DATA_A}"><script>var a=watch(document.getElementById('a'),'a');ev('inline:a',a);</script>`,
  // The error half: 404, zero bytes, bytes that are not an image.
  broken: `<img id=a src=/missing.png><img id=b src=/zero.png><img id=c src=/garbage.png>
    <script>['a','b','c'].forEach(function(n){watch(document.getElementById(n),n)});</script>`,
  // No src at all, and an empty one.
  no_src: `<img id=a><img id=b src=""><script>watch(document.getElementById('a'),'a');watch(document.getElementById('b'),'b');</script>`,
  // The window's load waits for a slow image of the document...
  before_winload: `<img id=a src=/a.png?delay=600><script>watch(document.getElementById('a'),'a');</script>`,
  // ...and for one a script inserted before it.
  inserted_before_winload: `<div id=box></div><script>document.addEventListener('DOMContentLoaded',function(){
    var i=watch(document.createElement('img'),'a');i.id='a';i.src='/a.png?delay=600';document.getElementById('box').appendChild(i);});</script>`,
  // An image added after the window's load.
  after_winload: `<div id=box></div><script>window.addEventListener('load',function(){setTimeout(function(){
    var i=watch(document.createElement('img'),'a');i.id='a';i.src='/a.png?delay=100';document.getElementById('box').appendChild(i);ev('added:a',i);},100);});</script>`,
  // What a load handler reads of the layout: the box has the image's size.
  measure: `<img id=a src=/a.png?delay=100><script>var a=document.getElementById('a');
    a.addEventListener('load',function(){var r=a.getBoundingClientRect();ev('a.load box='+r.width+'x'+r.height+' wh='+a.width+'x'+a.height,a);});</script>`,
  // The event itself.
  event_shape: `<img id=a src=/a.png?delay=100><img id=x src=/missing.png><script>
    function shape(e){ev(e.type+' target='+e.target.id+' bubbles='+e.bubbles+' cancelable='+e.cancelable+' trusted='+e.isTrusted+' ctor='+e.constructor.name+' this='+(this===e.target));}
    document.getElementById('a').addEventListener('load',shape);document.getElementById('x').addEventListener('error',shape);</script>`,
};

const reports = {};
const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1');
  if (url.pathname === '/report') {
    let body = '';
    req.on('data', (d) => { body += d; });
    req.on('end', () => {
      reports[url.searchParams.get('case')] = body;
      res.writeHead(204).end();
    });
    return;
  }
  const send = () => {
    if (url.pathname.startsWith('/case/')) {
      const name = url.pathname.slice(6);
      res.writeHead(200, { 'content-type': 'text/html', 'cache-control': 'no-store' });
      res.end(`<!doctype html><html><head><script>var CASE='${name}';</script>${HARNESS}</head><body>${CASES[name]}</body></html>`);
    } else if (FILES[url.pathname]) {
      res.writeHead(200, { 'content-type': FILES[url.pathname][0], 'cache-control': 'max-age=600' });
      res.end(FILES[url.pathname][1]);
    } else {
      res.writeHead(404, { 'content-type': 'text/plain' }).end('not found');
    }
  };
  const delay = Number(url.searchParams.get('delay') || 0);
  if (delay) setTimeout(send, delay); else send();
});
await new Promise((ok) => server.listen(0, '127.0.0.1', ok));
const origin = `http://127.0.0.1:${server.address().port}`;
const names = Object.keys(CASES).filter((n) => !only || only.includes(n));
const reported = (name, ms) => new Promise((ok) => {
  const started = Date.now();
  const poll = setInterval(() => {
    if (reports[name] !== undefined || Date.now() - started > ms) { clearInterval(poll); ok(); }
  }, 50);
});

if (rustkit) {
  console.log('rustkit ' + rustkit);
  const queue = names.slice();
  const lane = async () => {
    for (let name = queue.shift(); name; name = queue.shift()) {
      await new Promise((ok) => execFile(rustkit,
        ['--url', `${origin}/case/${name}`, '--width', '800', '--height', '600', '--live-ms', '9000', '--post-load-live'],
        { timeout: 60000 }, () => ok()));
    }
  };
  await Promise.all([lane(), lane(), lane(), lane()]);
} else {
  const { chromium } = await import('playwright');
  const browser = await chromium.launch({ headless: true, executablePath: process.env.PARITY_CHROME_PATH || undefined });
  console.log('chrome ' + browser.version());
  await Promise.all(names.map(async (name) => {
    const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
    await page.goto(`${origin}/case/${name}`, { waitUntil: 'commit' });
    await reported(name, 12000);
    await page.close();
  }));
  await browser.close();
}
for (const name of names) {
  console.log(`\n## ${name}\n${(reports[name] ?? '(no report)').split(' | ').join('\n')}`);
}
server.close();
