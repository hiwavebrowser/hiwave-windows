// Web API census probe. Measurement only: for each API, record whether it
// is missing, present but failing its smoke call, or present and working.
//
// Each entry: [area, name, path, smoke]. `path` is a dotted path from the
// global object; the API is "missing" when it resolves to undefined.
// `smoke` returns true (works), false (wrong result), throws (broken), or
// returns a Promise settling to one of those (resolved after the harness
// drains timers; a Promise still pending then is recorded as "pending").
(function () {
    var G = globalThis;
    var d = document;
    function el(tag) { return d.createElement(tag); }
    // `@name.member` paths start from a fresh instance instead of the global
    // object, so a method installed on instances (not on the interface's
    // prototype) still counts as present.
    var INSTANCES = {
        div: function () { return el('div'); },
        text: function () { return d.createTextNode('t'); },
        event: function () { return new Event('x'); },
        range: function () { return d.createRange(); },
        img: function () { return el('img'); },
        form: function () { return el('form'); },
        canvas: function () { return el('canvas'); },
        table: function () { return el('table'); },
        input: function () { return el('input'); },
        nodelist: function () { return d.querySelectorAll('li'); },
        usp: function () { return new URLSearchParams(''); },
        response: function () { return new Response(''); },
        blob: function () { return new Blob([]); },
        mo: function () { return new MutationObserver(function () {}); },
        sel: function () { return getSelection(); },
        sheet: function () { return d.styleSheets[0]; },
        a: function () { return d.getElementById('lnk'); }
    };
    ['template', 'select', 'button', 'label', 'video', 'dialog', 'details', 'iframe', 'link', 'style', 'script', 'textarea'].forEach(function (t) {
        INSTANCES[t] = function () { return el(t); };
    });
    function resolve(path) {
        var parts = path.split('.');
        var v = G;
        if (parts[0].charAt(0) === '@') {
            v = INSTANCES[parts.shift().slice(1)]();
        }
        for (var i = 0; i < parts.length; i++) {
            if (v === undefined || v === null) return undefined;
            v = v[parts[i]];
        }
        return v;
    }
    function later(fn) {
        // Promise resolved with fn's outcome; fn gets (done) callback.
        return new Promise(function (res, rej) { try { fn(res); } catch (e) { rej(e); } });
    }
    // A smoke check that can only confirm the member is callable (a real
    // call would need a network, a user gesture, a GPU or another window).
    function shallow(fn) { fn.shallow = true; return fn; }

    var E = [];
    function add(area, name, path, smoke) { E.push([area, name, path, smoke]); }

    // ---------------- Global / window ----------------
    var A = 'Window & globals';
    add(A, 'window', 'window', function () { return window === G || window.window === window; });
    add(A, 'self', 'self', function () { return self === window; });
    add(A, 'globalThis', 'globalThis', function () { return globalThis === window || globalThis.window === window; });
    add(A, 'window.top', 'top', function () { return top === window; });
    add(A, 'window.parent', 'parent', function () { return parent === window; });
    add(A, 'window.frames', 'frames', function () { return frames === window || typeof frames.length === 'number'; });
    add(A, 'window.innerWidth', 'innerWidth', function () { return typeof innerWidth === 'number' && innerWidth > 0; });
    add(A, 'window.innerHeight', 'innerHeight', function () { return typeof innerHeight === 'number' && innerHeight > 0; });
    add(A, 'window.devicePixelRatio', 'devicePixelRatio', function () { return devicePixelRatio > 0; });
    add(A, 'window.scrollX/scrollY', 'scrollY', function () { return scrollX === 0 && scrollY === 0; });
    add(A, 'window.pageYOffset', 'pageYOffset', function () { return pageYOffset === 0; });
    add(A, 'window.scrollTo', 'scrollTo', function () { scrollTo(0, 0); return true; });
    add(A, 'window.scrollBy', 'scrollBy', function () { scrollBy(0, 0); return true; });
    add(A, 'window.screen', 'screen', function () { return screen.width > 0 && screen.height > 0; });
    add(A, 'window.open', 'open', shallow(function () { return typeof open === 'function'; }));
    add(A, 'window.alert', 'alert', shallow(function () { return typeof alert === 'function'; }));
    add(A, 'window.confirm', 'confirm', shallow(function () { return typeof confirm === 'function'; }));
    add(A, 'window.postMessage', 'postMessage', function () { postMessage('x', '*'); return true; });
    add(A, 'window.addEventListener', 'addEventListener', function () { var n = 0; var f = function () { n++; }; addEventListener('census', f); dispatchEvent(new Event('census')); removeEventListener('census', f); return n === 1; });
    add(A, 'window.onerror (property)', 'onerror', shallow(function () { return 'onerror' in window; }));
    add(A, 'window.getSelection', 'getSelection', function () { return getSelection() !== undefined; });
    add(A, 'window.name', 'name', function () { return typeof window.name === 'string'; });
    add(A, 'window.origin', 'origin', function () { return typeof origin === 'string'; });
    add(A, 'window.isSecureContext', 'isSecureContext', function () { return typeof isSecureContext === 'boolean'; });
    add(A, 'window.frameElement', 'frameElement', function () { return frameElement === null; });
    add(A, 'window.visualViewport', 'visualViewport', function () { return visualViewport.width > 0; });
    add(A, 'atob', 'atob', function () { return atob('aGk=') === 'hi'; });
    add(A, 'btoa', 'btoa', function () { return btoa('hi') === 'aGk='; });
    add(A, 'reportError', 'reportError', shallow(function () { return typeof reportError === 'function'; }));

    // ---------------- Timers & scheduling ----------------
    A = 'Timers & scheduling';
    add(A, 'setTimeout', 'setTimeout', function () { return later(function (r) { setTimeout(function (a) { r(a === 7); }, 5, 7); }); });
    add(A, 'clearTimeout', 'clearTimeout', function () { var hit = false; var id = setTimeout(function () { hit = true; }, 1); clearTimeout(id); return later(function (r) { setTimeout(function () { r(!hit); }, 10); }); });
    add(A, 'setInterval', 'setInterval', function () { var n = 0; return later(function (r) { var id = setInterval(function () { if (++n === 3) { clearInterval(id); r(true); } }, 5); }); });
    add(A, 'clearInterval', 'clearInterval', function () { var id = setInterval(function () {}, 5); clearInterval(id); return true; });
    add(A, 'requestAnimationFrame', 'requestAnimationFrame', function () { return later(function (r) { requestAnimationFrame(function (t) { r(typeof t === 'number'); }); }); });
    add(A, 'cancelAnimationFrame', 'cancelAnimationFrame', function () { var id = requestAnimationFrame(function () {}); cancelAnimationFrame(id); return true; });
    add(A, 'requestIdleCallback', 'requestIdleCallback', function () { return later(function (r) { requestIdleCallback(function (dl) { r(typeof dl.timeRemaining() === 'number'); }); }); });
    add(A, 'cancelIdleCallback', 'cancelIdleCallback', function () { cancelIdleCallback(requestIdleCallback(function () {})); return true; });
    add(A, 'queueMicrotask', 'queueMicrotask', function () { return later(function (r) { queueMicrotask(function () { r(true); }); }); });
    add(A, 'scheduler.postTask', 'scheduler.postTask', function () { return scheduler.postTask(function () { return 3; }).then(function (v) { return v === 3; }); });
    add(A, 'MessageChannel', 'MessageChannel', function () { var c = new MessageChannel(); return later(function (r) { c.port2.onmessage = function (e) { r(e.data === 'm'); }; c.port1.postMessage('m'); }); });

    // ---------------- JS builtins (ECMAScript via Boa) ----------------
    A = 'JavaScript builtins';
    add(A, 'Promise', 'Promise', function () { return Promise.resolve(2).then(function (v) { return v === 2; }); });
    add(A, 'Promise.all', 'Promise.all', function () { return Promise.all([1, Promise.resolve(2)]).then(function (v) { return v.join() === '1,2'; }); });
    add(A, 'Promise.allSettled', 'Promise.allSettled', function () { return Promise.allSettled([Promise.reject(1)]).then(function (v) { return v[0].status === 'rejected'; }); });
    add(A, 'Promise.any', 'Promise.any', function () { return Promise.any([Promise.reject(1), 2]).then(function (v) { return v === 2; }); });
    add(A, 'Promise.withResolvers', 'Promise.withResolvers', function () { var p = Promise.withResolvers(); p.resolve(1); return p.promise.then(function (v) { return v === 1; }); });
    add(A, 'async/await', 'Promise', function () { return (0, eval)('(async function(){ return await Promise.resolve(4); })()').then(function (v) { return v === 4; }); });
    add(A, 'Map', 'Map', function () { return new Map([[1, 2]]).get(1) === 2; });
    add(A, 'Set', 'Set', function () { return new Set([1, 1, 2]).size === 2; });
    add(A, 'WeakMap', 'WeakMap', function () { var k = {}; var w = new WeakMap(); w.set(k, 1); return w.get(k) === 1; });
    add(A, 'WeakSet', 'WeakSet', function () { var k = {}; var w = new WeakSet([k]); return w.has(k); });
    add(A, 'WeakRef', 'WeakRef', function () { var k = {}; return new WeakRef(k).deref() === k; });
    add(A, 'FinalizationRegistry', 'FinalizationRegistry', function () { new FinalizationRegistry(function () {}).register({}, 1); return true; });
    add(A, 'Symbol', 'Symbol', function () { return typeof Symbol('x') === 'symbol' && typeof Symbol.iterator === 'symbol'; });
    add(A, 'Proxy', 'Proxy', function () { return new Proxy({}, { get: function () { return 5; } }).x === 5; });
    add(A, 'Reflect', 'Reflect', function () { return Reflect.ownKeys({ a: 1 }).join() === 'a'; });
    add(A, 'JSON', 'JSON', function () { return JSON.parse(JSON.stringify({ a: [1] })).a[0] === 1; });
    add(A, 'BigInt', 'BigInt', function () { return String(BigInt(2) ** BigInt(64)) === '18446744073709551616'; });
    add(A, 'ArrayBuffer', 'ArrayBuffer', function () { return new ArrayBuffer(8).byteLength === 8; });
    add(A, 'Uint8Array', 'Uint8Array', function () { return new Uint8Array([1, 2, 3]).subarray(1)[0] === 2; });
    add(A, 'Float64Array', 'Float64Array', function () { return new Float64Array([1.5])[0] === 1.5; });
    add(A, 'DataView', 'DataView', function () { var v = new DataView(new ArrayBuffer(4)); v.setUint16(0, 258); return v.getUint8(1) === 2; });
    add(A, 'SharedArrayBuffer', 'SharedArrayBuffer', function () { return new SharedArrayBuffer(4).byteLength === 4; });
    add(A, 'Atomics', 'Atomics', function () { var a = new Int32Array(new SharedArrayBuffer(4)); Atomics.add(a, 0, 2); return a[0] === 2; });
    add(A, 'Array.prototype.flat', 'Array.prototype.flat', function () { return [[1], [2, [3]]].flat(2).length === 3; });
    add(A, 'Array.prototype.at', 'Array.prototype.at', function () { return [1, 2].at(-1) === 2; });
    add(A, 'Array.prototype.findLast', 'Array.prototype.findLast', function () { return [1, 2, 3].findLast(function (x) { return x < 3; }) === 2; });
    add(A, 'Array.prototype.toSorted', 'Array.prototype.toSorted', function () { return [3, 1].toSorted().join() === '1,3'; });
    add(A, 'Array.from', 'Array.from', function () { return Array.from('ab').length === 2; });
    add(A, 'Object.fromEntries', 'Object.fromEntries', function () { return Object.fromEntries([['a', 1]]).a === 1; });
    add(A, 'Object.hasOwn', 'Object.hasOwn', function () { return Object.hasOwn({ a: 1 }, 'a'); });
    add(A, 'Object.groupBy', 'Object.groupBy', function () { return Object.groupBy([1, 2, 3], function (x) { return x % 2 ? 'o' : 'e'; }).o.length === 2; });
    add(A, 'String.prototype.replaceAll', 'String.prototype.replaceAll', function () { return 'aa'.replaceAll('a', 'b') === 'bb'; });
    add(A, 'String.prototype.padStart', 'String.prototype.padStart', function () { return '1'.padStart(3, '0') === '001'; });
    add(A, 'String.prototype.normalize', 'String.prototype.normalize', function () { return 'é'.normalize('NFC') === 'é'; });
    add(A, 'String.prototype.localeCompare', 'String.prototype.localeCompare', function () { return 'a'.localeCompare('b') < 0; });
    add(A, 'RegExp named groups', 'RegExp', function () { return /(?<y>\d+)/.exec('a12').groups.y === '12'; });
    add(A, 'RegExp lookbehind', 'RegExp', function () { return /(?<=\$)\d+/.exec('$42')[0] === '42'; });
    add(A, 'RegExp unicode property escapes', 'RegExp', function () { return /\p{L}+/u.test('héllo'); });
    add(A, 'Date', 'Date', function () { return new Date(0).toISOString() === '1970-01-01T00:00:00.000Z'; });
    add(A, 'Date.prototype.toLocaleDateString', 'Date.prototype.toLocaleDateString', function () { return typeof new Date(0).toLocaleDateString() === 'string'; });
    add(A, 'Number.prototype.toLocaleString', 'Number.prototype.toLocaleString', function () { return (1234.5).toLocaleString('en-US') === '1,234.5'; });
    add(A, 'Math', 'Math', function () { return Math.max(1, 3) === 3 && Math.hypot(3, 4) === 5; });
    add(A, 'globalThis.eval', 'eval', function () { return (0, eval)('1+1') === 2; });
    add(A, 'Function constructor', 'Function', function () { return new Function('a', 'return a*2')(3) === 6; });
    add(A, 'Generators', 'Function', function () { var g = (0, eval)('(function*(){ yield 1; yield 2; })')(); return g.next().value === 1; });
    add(A, 'Async iteration (for await)', 'Promise', function () { return (0, eval)('(async function(){ var s=0; for await (var x of [Promise.resolve(1),2]) s+=x; return s; })()').then(function (v) { return v === 3; }); });
    add(A, 'Optional chaining / nullish', 'Object', function () { return (0, eval)('var o=null; (o?.a ?? 5)') === 5; });
    add(A, 'Classes with private fields', 'Object', function () { return (0, eval)('(class { #x = 3; get x() { return this.#x; } })').prototype !== undefined && new ((0, eval)('(class { #x = 3; get x() { return this.#x; } })'))().x === 3; });
    add(A, 'Error.cause', 'Error', function () { return new Error('a', { cause: 1 }).cause === 1; });
    add(A, 'AggregateError', 'AggregateError', function () { return new AggregateError([1], 'm').errors.length === 1; });
    add(A, 'Array.prototype.includes', 'Array.prototype.includes', function () { return [NaN].includes(NaN); });
    add(A, 'structured string escape (encodeURIComponent)', 'encodeURIComponent', function () { return encodeURIComponent('a b&') === 'a%20b%26' && decodeURIComponent('%E2%82%AC') === '€'; });

    // ---------------- Intl ----------------
    A = 'Intl';
    add(A, 'Intl', 'Intl', function () { return typeof Intl === 'object'; });
    add(A, 'Intl.DateTimeFormat', 'Intl.DateTimeFormat', function () { return typeof new Intl.DateTimeFormat('en-US', { timeZone: 'UTC' }).format(new Date(0)) === 'string'; });
    add(A, 'Intl.NumberFormat', 'Intl.NumberFormat', function () { return new Intl.NumberFormat('en-US').format(1234.5) === '1,234.5'; });
    add(A, 'Intl.Collator', 'Intl.Collator', function () { return new Intl.Collator('en').compare('a', 'b') < 0; });
    add(A, 'Intl.PluralRules', 'Intl.PluralRules', function () { return new Intl.PluralRules('en-US').select(1) === 'one'; });
    add(A, 'Intl.RelativeTimeFormat', 'Intl.RelativeTimeFormat', function () { return new Intl.RelativeTimeFormat('en').format(-1, 'day') === '1 day ago'; });
    add(A, 'Intl.ListFormat', 'Intl.ListFormat', function () { return new Intl.ListFormat('en', { type: 'conjunction' }).format(['a', 'b', 'c']) === 'a, b, and c'; });
    add(A, 'Intl.Segmenter', 'Intl.Segmenter', function () { return Array.from(new Intl.Segmenter('en', { granularity: 'word' }).segment('hi there')).length >= 2; });
    add(A, 'Intl.Locale', 'Intl.Locale', function () { return new Intl.Locale('en-US').language === 'en'; });
    add(A, 'Intl.getCanonicalLocales', 'Intl.getCanonicalLocales', function () { return Intl.getCanonicalLocales('EN-us')[0] === 'en-US'; });

    // ---------------- Document ----------------
    A = 'Document';
    add(A, 'document', 'document', function () { return document.nodeType === 9; });
    add(A, 'document.documentElement', 'document.documentElement', function () { return d.documentElement.tagName === 'HTML'; });
    add(A, 'document.head', 'document.head', function () { return d.head.tagName === 'HEAD'; });
    add(A, 'document.body', 'document.body', function () { return d.body.tagName === 'BODY'; });
    add(A, 'document.title', 'document.title', function () { return d.title === 'Census'; });
    add(A, 'document.readyState', 'document.readyState', function () { return typeof d.readyState === 'string'; });
    add(A, 'document.URL', 'document.URL', function () { return d.URL.indexOf('https://census.test/') === 0; });
    add(A, 'document.location', 'document.location', function () { return d.location.href === location.href; });
    add(A, 'document.referrer', 'document.referrer', function () { return typeof d.referrer === 'string'; });
    add(A, 'document.domain', 'document.domain', function () { return d.domain === 'census.test'; });
    add(A, 'document.cookie', 'document.cookie', function () { d.cookie = 'k=v'; return typeof d.cookie === 'string'; });
    add(A, 'document.characterSet', 'document.characterSet', function () { return d.characterSet.toUpperCase() === 'UTF-8'; });
    add(A, 'document.compatMode', 'document.compatMode', function () { return d.compatMode === 'CSS1Compat'; });
    add(A, 'document.contentType', 'document.contentType', function () { return d.contentType === 'text/html'; });
    add(A, 'document.doctype', 'document.doctype', function () { return d.doctype && d.doctype.name === 'html'; });
    add(A, 'document.visibilityState', 'document.visibilityState', function () { return d.visibilityState === 'visible' || d.visibilityState === 'hidden'; });
    add(A, 'document.hidden', 'document.hidden', function () { return typeof d.hidden === 'boolean'; });
    add(A, 'document.hasFocus', 'document.hasFocus', function () { return typeof d.hasFocus() === 'boolean'; });
    add(A, 'document.activeElement', 'document.activeElement', function () { return d.activeElement === d.body || d.activeElement === null; });
    add(A, 'document.currentScript', 'document.currentScript', shallow(function () { return 'currentScript' in d; }));
    add(A, 'document.scripts', 'document.scripts', function () { return d.scripts.length === 0; });
    add(A, 'document.forms', 'document.forms', function () { return d.forms.length === 1; });
    add(A, 'document.images', 'document.images', function () { return d.images.length === 1; });
    add(A, 'document.links', 'document.links', function () { return d.links.length === 1; });
    add(A, 'document.getElementById', 'document.getElementById', function () { return d.getElementById('main').id === 'main'; });
    add(A, 'document.getElementsByClassName', 'document.getElementsByClassName', function () { return d.getElementsByClassName('item').length === 3; });
    add(A, 'document.getElementsByTagName', 'document.getElementsByTagName', function () { return d.getElementsByTagName('li').length === 3; });
    add(A, 'document.getElementsByName', 'document.getElementsByName', function () { return d.getElementsByName('q').length === 1; });
    add(A, 'document.querySelector', 'document.querySelector', function () { return d.querySelector('#main').id === 'main'; });
    add(A, 'document.querySelectorAll', 'document.querySelectorAll', function () { return d.querySelectorAll('.item').length === 3; });
    add(A, 'document.createElement', 'document.createElement', function () { return el('div').tagName === 'DIV'; });
    add(A, 'document.createElementNS', 'document.createElementNS', function () { var s = d.createElementNS('http://www.w3.org/2000/svg', 'svg'); return s.namespaceURI === 'http://www.w3.org/2000/svg'; });
    add(A, 'document.createTextNode', 'document.createTextNode', function () { return d.createTextNode('t').nodeType === 3; });
    add(A, 'document.createComment', 'document.createComment', function () { return d.createComment('c').nodeType === 8; });
    add(A, 'document.createDocumentFragment', 'document.createDocumentFragment', function () { var f = d.createDocumentFragment(); f.appendChild(el('i')); return f.nodeType === 11 && f.childNodes.length === 1; });
    add(A, 'document.createEvent', 'document.createEvent', function () { var e = d.createEvent('Event'); e.initEvent('x', true, true); return e.type === 'x' && e.bubbles; });
    add(A, 'document.createRange', 'document.createRange', function () { return d.createRange().collapsed === true; });
    add(A, 'document.createTreeWalker', 'document.createTreeWalker', function () { var w = d.createTreeWalker(d.getElementById('list'), 1); return w.nextNode().tagName === 'LI'; });
    add(A, 'document.createNodeIterator', 'document.createNodeIterator', function () { var it = d.createNodeIterator(d.getElementById('list'), 1); return it.nextNode().id === 'list'; });
    add(A, 'document.importNode', 'document.importNode', function () { return d.importNode(el('p'), true).tagName === 'P'; });
    add(A, 'document.adoptNode', 'document.adoptNode', function () { return d.adoptNode(el('p')).ownerDocument === d; });
    add(A, 'document.implementation.createHTMLDocument', 'document.implementation.createHTMLDocument', function () { var doc = d.implementation.createHTMLDocument('x'); return doc.title === 'x' && doc.body !== null; });
    add(A, 'document.elementFromPoint', 'document.elementFromPoint', function () { var r = d.elementFromPoint(1, 1); return r === null || r.nodeType === 1; });
    add(A, 'document.write', 'document.write', shallow(function () { return typeof d.write === 'function'; }));
    add(A, 'document.execCommand', 'document.execCommand', function () { return typeof d.execCommand('copy') === 'boolean'; });
    add(A, 'document.fonts', 'document.fonts', function () { return typeof d.fonts.ready.then === 'function'; });
    add(A, 'document.styleSheets', 'document.styleSheets', function () { return typeof d.styleSheets.length === 'number'; });
    add(A, 'document.adoptedStyleSheets', 'document.adoptedStyleSheets', function () { return Array.isArray(d.adoptedStyleSheets); });
    add(A, 'document.startViewTransition', 'document.startViewTransition', shallow(function () { return typeof d.startViewTransition === 'function'; }));
    add(A, 'document.fullscreenElement', 'document.exitFullscreen', function () { return d.fullscreenElement === null; });
    add(A, 'DOMContentLoaded listener', 'document.addEventListener', function () { d.addEventListener('DOMContentLoaded', function () {}); return true; });

    // ---------------- Node & tree ----------------
    A = 'Node & tree mutation';
    add(A, 'Node (interface)', 'Node', function () { return Node.ELEMENT_NODE === 1 && d.body instanceof Node; });
    add(A, 'appendChild', '@div.appendChild', function () { var p = el('div'); var c = el('span'); return p.appendChild(c) === c && c.parentNode === p; });
    add(A, 'insertBefore', '@div.insertBefore', function () { var p = el('div'); var a = el('a'); var b = el('b'); p.appendChild(b); p.insertBefore(a, b); return p.firstChild === a; });
    add(A, 'removeChild', '@div.removeChild', function () { var p = el('div'); var c = p.appendChild(el('i')); p.removeChild(c); return p.childNodes.length === 0; });
    add(A, 'replaceChild', '@div.replaceChild', function () { var p = el('div'); var a = p.appendChild(el('a')); var b = el('b'); p.replaceChild(b, a); return p.firstChild === b; });
    add(A, 'cloneNode(deep)', '@div.cloneNode', function () { var p = el('div'); p.appendChild(el('i')).textContent = 'x'; var c = p.cloneNode(true); return c !== p && c.firstChild.textContent === 'x'; });
    add(A, 'contains', '@div.contains', function () { return d.body.contains(d.getElementById('main')); });
    add(A, 'hasChildNodes', '@div.hasChildNodes', function () { return d.body.hasChildNodes(); });
    add(A, 'isEqualNode', '@div.isEqualNode', function () { return el('p').isEqualNode(el('p')); });
    add(A, 'isSameNode', '@div.isSameNode', function () { return d.body.isSameNode(d.body); });
    add(A, 'compareDocumentPosition', '@div.compareDocumentPosition', function () { return (d.head.compareDocumentPosition(d.body) & 4) === 4; });
    add(A, 'normalize', '@div.normalize', function () { var p = el('p'); p.appendChild(d.createTextNode('a')); p.appendChild(d.createTextNode('b')); p.normalize(); return p.childNodes.length === 1; });
    add(A, 'getRootNode', '@div.getRootNode', function () { return d.body.getRootNode() === d; });
    add(A, 'textContent', '@div.textContent', function () { var p = el('p'); p.textContent = 'hi'; return p.textContent === 'hi' && p.firstChild.nodeType === 3; });
    add(A, 'nodeName/nodeType/nodeValue', 'document.body.nodeName', function () { var t = d.createTextNode('v'); return d.body.nodeName === 'BODY' && t.nodeValue === 'v'; });
    add(A, 'parentNode/parentElement', 'document.body.parentNode', function () { return d.body.parentElement === d.documentElement; });
    add(A, 'childNodes/firstChild/lastChild', 'document.body.childNodes', function () { var l = d.getElementById('list'); return l.firstElementChild.tagName === 'LI' && l.childNodes.length >= 3; });
    add(A, 'nextSibling/previousSibling', 'document.body.firstChild', function () { var a = d.getElementById('a1'); return a.nextElementSibling.id === 'a2' && a.nextElementSibling.previousElementSibling === a; });
    add(A, 'isConnected', 'document.body.isConnected', function () { return d.body.isConnected === true && el('i').isConnected === false; });
    add(A, 'ownerDocument', 'document.body.ownerDocument', function () { return el('i').ownerDocument === d; });
    add(A, 'NodeList.prototype.forEach', '@nodelist.forEach', function () { var n = 0; d.querySelectorAll('.item').forEach(function () { n++; }); return n === 3; });
    add(A, 'NodeList iterable (for-of)', 'NodeList', function () { var n = 0; for (var x of d.querySelectorAll('.item')) n++; return n === 3; });
    add(A, 'HTMLCollection.item/namedItem', 'HTMLCollection', function () { var c = d.getElementsByTagName('li'); return c.item(0) === c[0]; });
    add(A, 'Text.splitText', '@text.splitText', function () { var t = d.createTextNode('abcd'); el('p').appendChild(t); return t.splitText(2).data === 'cd' && t.data === 'ab'; });
    add(A, 'CharacterData.appendData', '@text.appendData', function () { var t = d.createTextNode('a'); t.appendData('b'); return t.data === 'ab'; });
    add(A, 'DocumentFragment (ctor)', 'DocumentFragment', function () { return new DocumentFragment().nodeType === 11; });
    add(A, 'Text (ctor)', 'Text', function () { return new Text('x').data === 'x'; });
    add(A, 'Comment (ctor)', 'Comment', function () { return new Comment('x').data === 'x'; });

    // ---------------- Element ----------------
    A = 'Element';
    function m() { return d.getElementById('main'); }
    add(A, 'Element (interface)', 'Element', function () { return m() instanceof Element; });
    add(A, 'HTMLElement (interface)', 'HTMLElement', function () { return m() instanceof HTMLElement; });
    add(A, 'id/className/tagName', '@div.tagName', function () { return m().id === 'main' && m().className === 'box wide' && m().tagName === 'DIV'; });
    add(A, 'getAttribute/setAttribute', '@div.setAttribute', function () { var e = el('div'); e.setAttribute('data-x', '1'); return e.getAttribute('data-x') === '1'; });
    add(A, 'removeAttribute/hasAttribute', '@div.removeAttribute', function () { var e = el('div'); e.setAttribute('a', ''); e.removeAttribute('a'); return !e.hasAttribute('a'); });
    add(A, 'toggleAttribute', '@div.toggleAttribute', function () { var e = el('div'); e.toggleAttribute('hidden'); return e.hasAttribute('hidden'); });
    add(A, 'getAttributeNames', '@div.getAttributeNames', function () { var e = el('div'); e.setAttribute('a', '1'); return e.getAttributeNames().join() === 'a'; });
    add(A, 'attributes (NamedNodeMap)', '@div.attributes', function () { var e = el('div'); e.setAttribute('a', '1'); return e.attributes.length === 1 && e.attributes[0].name === 'a'; });
    add(A, 'classList.add/remove/contains', '@div.classList', function () { var e = el('div'); e.classList.add('a', 'b'); e.classList.remove('a'); return e.className === 'b' && e.classList.contains('b'); });
    add(A, 'classList.toggle/replace', '@div.classList.toggle', function () { var e = el('div'); e.classList.toggle('x'); e.classList.replace('x', 'y'); return e.className === 'y'; });
    add(A, 'dataset', '@div.dataset', function () { var e = m(); e.dataset.fooBar = 'z'; return e.getAttribute('data-foo-bar') === 'z' && m().dataset.role === 'hero'; });
    add(A, 'style (inline CSSStyleDeclaration)', '@div.style', function () { var e = el('div'); e.style.color = 'red'; e.style.setProperty('margin-top', '4px'); return e.style.color === 'red' && e.style.getPropertyValue('margin-top') === '4px'; });
    add(A, 'style.cssText', '@div.style', function () { var e = el('div'); e.style.cssText = 'width: 3px'; return e.style.width === '3px'; });
    add(A, 'innerHTML (get/set)', '@div.innerHTML', function () { var e = el('div'); e.innerHTML = '<b>x</b><i>y</i>'; return e.children.length === 2 && e.innerHTML === '<b>x</b><i>y</i>'; });
    add(A, 'outerHTML', '@div.outerHTML', function () { var e = el('p'); e.textContent = 'x'; return e.outerHTML === '<p>x</p>'; });
    add(A, 'innerText', '@div.innerText', function () { var e = el('p'); e.innerText = 'x'; return e.textContent === 'x'; });
    add(A, 'insertAdjacentHTML', '@div.insertAdjacentHTML', function () { var e = el('div'); e.insertAdjacentHTML('beforeend', '<i>1</i>'); e.insertAdjacentHTML('afterbegin', '<b>0</b>'); return e.firstChild.tagName === 'B' && e.lastChild.tagName === 'I'; });
    add(A, 'insertAdjacentElement', '@div.insertAdjacentElement', function () { var e = el('div'); e.insertAdjacentElement('beforeend', el('i')); return e.firstChild.tagName === 'I'; });
    add(A, 'insertAdjacentText', '@div.insertAdjacentText', function () { var e = el('div'); e.insertAdjacentText('beforeend', 't'); return e.textContent === 't'; });
    add(A, 'append/prepend', '@div.append', function () { var e = el('div'); e.append('b', el('i')); e.prepend(el('a')); return e.childNodes.length === 3 && e.firstChild.tagName === 'A'; });
    add(A, 'before/after', '@div.before', function () { var p = el('div'); var c = p.appendChild(el('i')); c.before(el('a')); c.after(el('b')); return p.children.length === 3 && p.lastChild.tagName === 'B'; });
    add(A, 'remove', '@div.remove', function () { var p = el('div'); var c = p.appendChild(el('i')); c.remove(); return p.childNodes.length === 0; });
    add(A, 'replaceWith', '@div.replaceWith', function () { var p = el('div'); var c = p.appendChild(el('i')); c.replaceWith(el('b')); return p.firstChild.tagName === 'B'; });
    add(A, 'replaceChildren', '@div.replaceChildren', function () { var p = el('div'); p.appendChild(el('i')); p.replaceChildren(el('a'), el('b')); return p.children.length === 2; });
    add(A, 'children/childElementCount', '@div.children', function () { return d.getElementById('list').children.length === 3 && d.getElementById('list').childElementCount === 3; });
    add(A, 'matches', '@div.matches', function () { return m().matches('.box') && !m().matches('.nope'); });
    add(A, 'closest', '@div.closest', function () { return d.getElementById('a1').closest('#list').id === 'list'; });
    add(A, 'Element.querySelector (scoped)', '@div.querySelector', function () { return d.getElementById('list').querySelector('.item').id === 'a1'; });
    add(A, 'Element.querySelectorAll (scoped)', '@div.querySelectorAll', function () { return d.getElementById('list').querySelectorAll('li').length === 3; });
    add(A, 'querySelector complex selector', 'document.querySelector', function () { var r = d.querySelector('ul#list > li.item:nth-child(2)'); return r !== null && r.id === 'a2'; });
    add(A, 'querySelector attribute selector', 'document.querySelector', function () { var r = d.querySelector('[data-role="hero"]'); return r !== null && r.id === 'main'; });
    add(A, 'getElementsByClassName (Element)', '@div.getElementsByClassName', function () { return d.getElementById('list').getElementsByClassName('item').length === 3; });
    add(A, 'getBoundingClientRect', '@div.getBoundingClientRect', function () { var r = m().getBoundingClientRect(); return typeof r.width === 'number' && typeof r.top === 'number'; });
    add(A, 'getClientRects', '@div.getClientRects', function () { return typeof m().getClientRects().length === 'number'; });
    add(A, 'offsetWidth/offsetHeight/offsetTop', '@div.offsetWidth', function () { return typeof m().offsetWidth === 'number' && typeof m().offsetTop === 'number'; });
    add(A, 'offsetParent', '@div.offsetParent', shallow(function () { return 'offsetParent' in m(); }));
    add(A, 'clientWidth/clientHeight', '@div.clientWidth', function () { return typeof m().clientWidth === 'number'; });
    add(A, 'scrollWidth/scrollHeight/scrollTop', '@div.scrollHeight', function () { return typeof m().scrollHeight === 'number' && typeof m().scrollTop === 'number'; });
    add(A, 'scrollIntoView', '@div.scrollIntoView', function () { m().scrollIntoView({ block: 'nearest' }); return true; });
    add(A, 'Element.scrollTo', '@div.scrollTo', function () { m().scrollTo(0, 0); return true; });
    add(A, 'focus/blur', '@div.focus', function () { var i = d.getElementById('q'); i.focus(); var ok = d.activeElement === i; i.blur(); return ok; });
    add(A, 'click()', '@div.click', function () { var n = 0; var b = el('button'); b.addEventListener('click', function () { n++; }); b.click(); return n === 1; });
    add(A, 'hidden property', '@div.hidden', function () { var e = el('div'); e.hidden = true; return e.hasAttribute('hidden'); });
    add(A, 'tabIndex/title/lang/dir', '@div.tabIndex', function () { var e = el('div'); e.title = 't'; e.tabIndex = 2; return e.getAttribute('title') === 't' && e.getAttribute('tabindex') === '2'; });
    add(A, 'contentEditable/isContentEditable', '@div.contentEditable', function () { var e = el('div'); e.contentEditable = 'true'; return e.getAttribute('contenteditable') === 'true'; });
    add(A, 'attachShadow', '@div.attachShadow', function () { var e = el('div'); var s = e.attachShadow({ mode: 'open' }); s.innerHTML = '<p>x</p>'; return e.shadowRoot === s && s.host === e && s.childNodes.length === 1; });
    add(A, 'ShadowRoot (interface)', '@div.attachShadow', function () { return el('div').attachShadow({ mode: 'open' }) instanceof ShadowRoot; });
    add(A, 'slot / assignedNodes', '@div.attachShadow', function () { var h = el('div'); h.appendChild(el('i')); var s = h.attachShadow({ mode: 'open' }); s.innerHTML = '<slot></slot>'; return s.firstChild.assignedNodes().length === 1; });
    add(A, 'animate (Web Animations)', '@div.animate', function () { var a = m().animate([{ opacity: 0 }, { opacity: 1 }], 10); return typeof a.cancel === 'function'; });
    add(A, 'getAnimations', '@div.getAnimations', function () { return Array.isArray(m().getAnimations()); });
    add(A, 'requestFullscreen', '@div.requestFullscreen', shallow(function () { return typeof m().requestFullscreen === 'function'; }));
    add(A, 'setPointerCapture', '@div.setPointerCapture', shallow(function () { return typeof m().setPointerCapture === 'function'; }));
    add(A, 'checkVisibility', '@div.checkVisibility', function () { return typeof m().checkVisibility() === 'boolean'; });
    add(A, 'HTMLElement.prototype.popover', '@div.showPopover', shallow(function () { return typeof el('div').showPopover === 'function'; }));
    add(A, 'Element.prototype.computedStyleMap', '@div.computedStyleMap', function () { return typeof m().computedStyleMap().get === 'function'; });

    // ---------------- HTML element specifics ----------------
    A = 'HTML elements';
    add(A, 'template.content', '@template.content', function () { var t = el('template'); t.innerHTML = '<p>x</p>'; return t.content.nodeType === 11 && t.content.firstChild.tagName === 'P' && t.childNodes.length === 0; });
    add(A, 'HTMLAnchorElement.href (resolved)', '@a.href', function () { return d.getElementById('lnk').href === 'https://census.test/page?x=1'; });
    add(A, 'HTMLAnchorElement URL parts', '@a.pathname', function () { var a = d.getElementById('lnk'); return a.pathname === '/page' && a.search === '?x=1' && a.hostname === 'census.test'; });
    add(A, 'HTMLImageElement / new Image()', 'Image', function () { var i = new Image(4, 5); return i.tagName === 'IMG' && i.width === 4; });
    add(A, 'img.complete/naturalWidth', '@img.complete', function () { var i = d.images[0]; return typeof i.complete === 'boolean' && typeof i.naturalWidth === 'number'; });
    add(A, 'img.decode', '@img.decode', shallow(function () { return typeof new Image().decode === 'function'; }));
    add(A, 'input.value', '@input.value', function () { var i = d.getElementById('q'); i.value = 'abc'; return i.value === 'abc'; });
    add(A, 'input.checked', '@input.checked', function () { var i = el('input'); i.type = 'checkbox'; i.checked = true; return i.checked === true; });
    add(A, 'input.setSelectionRange', '@input.setSelectionRange', function () { var i = d.getElementById('q'); i.value = 'abcd'; i.setSelectionRange(1, 3); return i.selectionStart === 1 && i.selectionEnd === 3; });
    add(A, 'input.validity/checkValidity', '@input.checkValidity', function () { var i = el('input'); i.required = true; return i.checkValidity() === false && i.validity.valueMissing === true; });
    add(A, 'input.files', '@input.files', function () { var i = el('input'); i.type = 'file'; return i.files !== undefined; });
    add(A, 'textarea.value', '@textarea.value', function () { var t = el('textarea'); t.value = 'x'; return t.value === 'x'; });
    add(A, 'select.value/options/selectedIndex', '@select.options', function () { var s = d.getElementById('sel'); return s.options.length === 2 && s.value === 'b' && s.selectedIndex === 1; });
    add(A, 'new Option()', 'Option', function () { var o = new Option('t', 'v'); return o.value === 'v' && o.text === 't'; });
    add(A, 'form.elements', '@form.elements', function () { return d.getElementById('f').elements.length >= 2; });
    add(A, 'form.submit/requestSubmit', '@form.requestSubmit', function () { var f = d.getElementById('f'); var n = 0; f.addEventListener('submit', function (e) { e.preventDefault(); n++; }); f.requestSubmit(); return n === 1; });
    add(A, 'form.reset', '@form.reset', function () { d.getElementById('f').reset(); return true; });
    add(A, 'button.type/disabled', '@button.type', function () { var b = el('button'); b.disabled = true; return b.type === 'submit' && b.hasAttribute('disabled'); });
    add(A, 'label.htmlFor/control', '@label.htmlFor', function () { var l = d.getElementById('lab'); return l.htmlFor === 'q' && l.control === d.getElementById('q'); });
    add(A, 'canvas.getContext("2d")', '@canvas.getContext', function () { var c = el('canvas'); var ctx = c.getContext('2d'); ctx.fillRect(0, 0, 1, 1); return ctx !== null; });
    add(A, 'canvas.toDataURL', '@canvas.toDataURL', function () { return el('canvas').toDataURL().indexOf('data:image/png') === 0; });
    add(A, 'video/audio play()/paused', '@video.paused', function () { var v = el('video'); return v.paused === true && typeof v.play === 'function'; });
    add(A, 'dialog.showModal/close', '@dialog.showModal', function () { var g = el('dialog'); d.body.appendChild(g); g.showModal(); var ok = g.open; g.close(); g.remove(); return ok && !g.open; });
    add(A, 'details.open', '@details.open', function () { var x = el('details'); x.open = true; return x.hasAttribute('open'); });
    add(A, 'iframe.contentWindow', '@iframe.contentWindow', function () { var f = el('iframe'); return 'contentWindow' in f; });
    add(A, 'script element (createElement)', '@script.src', function () { var s = el('script'); s.src = '/x.js'; s.async = true; return s.src === 'https://census.test/x.js'; });
    add(A, 'link rel=stylesheet element', '@link.rel', function () { var l = el('link'); l.rel = 'stylesheet'; return l.getAttribute('rel') === 'stylesheet'; });
    add(A, 'style element .sheet', '@style.sheet', function () { var s = el('style'); s.textContent = 'p{color:red}'; d.head.appendChild(s); var ok = s.sheet !== null && s.sheet !== undefined; s.remove(); return ok; });
    add(A, 'table.insertRow/insertCell', '@table.insertRow', function () { var t = el('table'); var r = t.insertRow(); r.insertCell(); return t.rows.length === 1 && r.cells.length === 1; });
    add(A, 'SVGElement / createElementNS svg', 'document.createElementNS', function () { return d.createElementNS('http://www.w3.org/2000/svg', 'circle') instanceof SVGElement; });

    // ---------------- Events ----------------
    A = 'Events';
    add(A, 'EventTarget (ctor)', 'EventTarget', function () { var t = new EventTarget(); var n = 0; t.addEventListener('x', function () { n++; }); t.dispatchEvent(new Event('x')); return n === 1; });
    add(A, 'Event (ctor)', 'Event', function () { var e = new Event('x', { bubbles: true, cancelable: true }); return e.type === 'x' && e.bubbles && e.cancelable; });
    add(A, 'CustomEvent', 'CustomEvent', function () { var got; var e = el('div'); e.addEventListener('c', function (ev) { got = ev.detail.v; }); e.dispatchEvent(new CustomEvent('c', { detail: { v: 3 } })); return got === 3; });
    add(A, 'bubbling + currentTarget', 'Event', function () { var p = el('div'); var c = p.appendChild(el('i')); var t; p.addEventListener('x', function (e) { t = e.currentTarget === p && e.target === c; }); c.dispatchEvent(new Event('x', { bubbles: true })); return t === true; });
    add(A, 'capture phase', 'Event', function () { var p = el('div'); var c = p.appendChild(el('i')); var log = []; p.addEventListener('x', function () { log.push('cap'); }, true); c.addEventListener('x', function () { log.push('tgt'); }); c.dispatchEvent(new Event('x', { bubbles: true })); return log.join() === 'cap,tgt'; });
    add(A, 'stopPropagation', '@event.stopPropagation', function () { var p = el('div'); var c = p.appendChild(el('i')); var hit = false; p.addEventListener('x', function () { hit = true; }); c.addEventListener('x', function (e) { e.stopPropagation(); }); c.dispatchEvent(new Event('x', { bubbles: true })); return !hit; });
    add(A, 'stopImmediatePropagation', '@event.stopImmediatePropagation', function () { var e1 = el('i'); var n = 0; e1.addEventListener('x', function (e) { e.stopImmediatePropagation(); n++; }); e1.addEventListener('x', function () { n++; }); e1.dispatchEvent(new Event('x')); return n === 1; });
    add(A, 'preventDefault/defaultPrevented', '@event.preventDefault', function () { var e1 = el('i'); e1.addEventListener('x', function (e) { e.preventDefault(); }); var r = e1.dispatchEvent(new Event('x', { cancelable: true })); return r === false; });
    add(A, 'addEventListener once', 'EventTarget', function () { var e1 = el('i'); var n = 0; e1.addEventListener('x', function () { n++; }, { once: true }); e1.dispatchEvent(new Event('x')); e1.dispatchEvent(new Event('x')); return n === 1; });
    add(A, 'addEventListener signal', 'EventTarget', function () { var e1 = el('i'); var n = 0; var c = new AbortController(); e1.addEventListener('x', function () { n++; }, { signal: c.signal }); c.abort(); e1.dispatchEvent(new Event('x')); return n === 0; });
    add(A, 'handleEvent object listener', 'EventTarget', function () { var e1 = el('i'); var n = 0; e1.addEventListener('x', { handleEvent: function () { n++; } }); e1.dispatchEvent(new Event('x')); return n === 1; });
    add(A, 'on* handler property', '@button.onclick', function () { var b = el('button'); var n = 0; b.onclick = function () { n++; }; b.click(); return n === 1; });
    add(A, 'composedPath', '@event.composedPath', function () { var p = el('div'); var c = p.appendChild(el('i')); var len; c.addEventListener('x', function (e) { len = e.composedPath().length; }); c.dispatchEvent(new Event('x', { bubbles: true })); return len >= 2; });
    add(A, 'MouseEvent', 'MouseEvent', function () { return new MouseEvent('click', { clientX: 3 }).clientX === 3; });
    add(A, 'KeyboardEvent', 'KeyboardEvent', function () { return new KeyboardEvent('keydown', { key: 'a' }).key === 'a'; });
    add(A, 'PointerEvent', 'PointerEvent', function () { return new PointerEvent('pointerdown', { pointerId: 2 }).pointerId === 2; });
    add(A, 'TouchEvent', 'TouchEvent', function () { return new TouchEvent('touchstart').type === 'touchstart'; });
    add(A, 'FocusEvent', 'FocusEvent', function () { return new FocusEvent('focus').type === 'focus'; });
    add(A, 'InputEvent', 'InputEvent', function () { return new InputEvent('input', { data: 'a' }).data === 'a'; });
    add(A, 'WheelEvent', 'WheelEvent', function () { return new WheelEvent('wheel', { deltaY: 1 }).deltaY === 1; });
    add(A, 'MessageEvent', 'MessageEvent', function () { return new MessageEvent('message', { data: 1 }).data === 1; });
    add(A, 'ErrorEvent', 'ErrorEvent', function () { return new ErrorEvent('error', { message: 'm' }).message === 'm'; });
    add(A, 'SubmitEvent', 'SubmitEvent', function () { return new SubmitEvent('submit').type === 'submit'; });
    add(A, 'AnimationEvent/TransitionEvent', 'TransitionEvent', function () { return new TransitionEvent('transitionend', { propertyName: 'opacity' }).propertyName === 'opacity'; });
    add(A, 'ClipboardEvent', 'ClipboardEvent', function () { return new ClipboardEvent('paste').type === 'paste'; });
    add(A, 'DragEvent', 'DragEvent', function () { return new DragEvent('drop').type === 'drop'; });
    add(A, 'PromiseRejectionEvent', 'PromiseRejectionEvent', function () { var p = Promise.reject(1); p.catch(function () {}); return new PromiseRejectionEvent('unhandledrejection', { promise: p, reason: 1 }).reason === 1; });

    // ---------------- Observers ----------------
    A = 'Observers';
    add(A, 'MutationObserver (childList)', 'MutationObserver', function () { var t = el('div'); d.body.appendChild(t); return later(function (r) { var mo = new MutationObserver(function (recs) { mo.disconnect(); t.remove(); r(recs[0].type === 'childList' && recs[0].addedNodes.length === 1); }); mo.observe(t, { childList: true }); t.appendChild(el('i')); }); });
    add(A, 'MutationObserver (attributes)', 'MutationObserver', function () { var t = el('div'); return later(function (r) { var mo = new MutationObserver(function (recs) { mo.disconnect(); r(recs[0].attributeName === 'data-a' && recs[0].oldValue === '1'); }); t.setAttribute('data-a', '1'); mo.observe(t, { attributes: true, attributeOldValue: true }); t.setAttribute('data-a', '2'); }); });
    add(A, 'MutationObserver (subtree characterData)', 'MutationObserver', function () { var t = el('div'); var tx = t.appendChild(el('p')).appendChild(d.createTextNode('a')); return later(function (r) { var mo = new MutationObserver(function (recs) { mo.disconnect(); r(recs[0].type === 'characterData'); }); mo.observe(t, { characterData: true, subtree: true }); tx.data = 'b'; }); });
    add(A, 'MutationObserver.takeRecords', '@mo.takeRecords', function () { var t = el('div'); var mo = new MutationObserver(function () {}); mo.observe(t, { childList: true }); t.appendChild(el('i')); var n = mo.takeRecords().length; mo.disconnect(); return n === 1; });
    add(A, 'IntersectionObserver', 'IntersectionObserver', function () { var io = new IntersectionObserver(function () {}); io.observe(m()); io.unobserve(m()); io.disconnect(); return true; });
    add(A, 'IntersectionObserver callback fires', 'IntersectionObserver', function () { return later(function (r) { var io = new IntersectionObserver(function (es) { io.disconnect(); r(es.length === 1 && typeof es[0].isIntersecting === 'boolean'); }); io.observe(m()); }); });
    add(A, 'ResizeObserver', 'ResizeObserver', function () { var ro = new ResizeObserver(function () {}); ro.observe(m()); ro.disconnect(); return true; });
    add(A, 'ResizeObserver callback fires', 'ResizeObserver', function () { return later(function (r) { var ro = new ResizeObserver(function (es) { ro.disconnect(); r(es.length === 1 && es[0].contentRect !== undefined); }); ro.observe(m()); }); });
    add(A, 'PerformanceObserver', 'PerformanceObserver', function () { var po = new PerformanceObserver(function () {}); po.observe({ type: 'mark' }); po.disconnect(); return true; });
    add(A, 'ReportingObserver', 'ReportingObserver', function () { new ReportingObserver(function () {}).disconnect(); return true; });

    // ---------------- Web Components ----------------
    A = 'Web Components';
    add(A, 'customElements.define', 'customElements.define', function () { var C = (0, eval)('(class extends HTMLElement { connectedCallback() { this.dataset.up = "1"; } })'); customElements.define('census-el', C); var e = el('census-el'); d.body.appendChild(e); var ok = e instanceof C && e.dataset.up === '1'; e.remove(); return ok; });
    add(A, 'customElements.get', 'customElements.get', function () { return customElements.get('census-nope') === undefined; });
    add(A, 'customElements.whenDefined', 'customElements.whenDefined', function () { var p = customElements.whenDefined('census-late'); customElements.define('census-late', (0, eval)('(class extends HTMLElement {})')); return p.then(function () { return true; }); });
    add(A, 'attributeChangedCallback', 'customElements', function () { var log = []; var C = (0, eval)('(function(log){ return class extends HTMLElement { static get observedAttributes() { return ["v"]; } attributeChangedCallback(n, o, v) { log.push(n + "=" + v); } }; })')(log); customElements.define('census-attr', C); var e = el('census-attr'); e.setAttribute('v', '2'); return log.join() === 'v=2'; });
    add(A, 'customElements.upgrade', 'customElements.upgrade', function () { customElements.upgrade(el('div')); return true; });
    add(A, 'ElementInternals (attachInternals)', '@div.attachInternals', shallow(function () { return typeof HTMLElement.prototype.attachInternals === 'function'; }));
    add(A, 'CSSStyleSheet constructable', 'CSSStyleSheet', function () { var s = new CSSStyleSheet(); s.replaceSync('p{color:red}'); return s.cssRules.length === 1; });

    // ---------------- Networking ----------------
    A = 'Networking';
    add(A, 'fetch (returns Promise)', 'fetch', function () { var p = fetch('/census.json'); p.catch(function () {}); return typeof p.then === 'function'; });
    add(A, 'Request', 'Request', function () { var r = new Request('/a', { method: 'POST' }); return r.method === 'POST' && r.url === 'https://census.test/a'; });
    add(A, 'Response', 'Response', function () { var r = new Response('x', { status: 201 }); return r.status === 201 && r.ok; });
    add(A, 'Response.text', '@response.text', function () { return new Response('body').text().then(function (t) { return t === 'body'; }); });
    add(A, 'Response.json', '@response.json', function () { return new Response('{"a":1}').json().then(function (j) { return j.a === 1; }); });
    add(A, 'Response.json (static)', 'Response.json', function () { return Response.json({ a: 1 }).json().then(function (j) { return j.a === 1; }); });
    add(A, 'Response.arrayBuffer', '@response.arrayBuffer', function () { return new Response('ab').arrayBuffer().then(function (b) { return b.byteLength === 2; }); });
    add(A, 'Response.blob', '@response.blob', function () { return new Response('ab').blob().then(function (b) { return b.size === 2; }); });
    add(A, 'Headers', 'Headers', function () { var h = new Headers({ 'Content-Type': 'text/plain' }); h.append('X-A', '1'); return h.get('content-type') === 'text/plain' && h.has('x-a'); });
    add(A, 'XMLHttpRequest', 'XMLHttpRequest', function () { var x = new XMLHttpRequest(); x.open('GET', '/a'); return x.readyState === 1; });
    add(A, 'AbortController', 'AbortController', function () { var c = new AbortController(); var n = 0; c.signal.addEventListener('abort', function () { n++; }); c.abort(); return c.signal.aborted && n === 1; });
    add(A, 'AbortSignal.timeout', 'AbortSignal.timeout', function () { var s = AbortSignal.timeout(5); return later(function (r) { setTimeout(function () { r(s.aborted); }, 20); }); });
    add(A, 'AbortSignal.any', 'AbortSignal.any', function () { var c = new AbortController(); var s = AbortSignal.any([c.signal]); c.abort(); return s.aborted; });
    add(A, 'WebSocket', 'WebSocket', shallow(function () { return typeof WebSocket === 'function' && WebSocket.OPEN === 1; }));
    add(A, 'EventSource', 'EventSource', shallow(function () { return typeof EventSource === 'function'; }));
    add(A, 'navigator.sendBeacon', 'navigator.sendBeacon', function () { return typeof navigator.sendBeacon('/b', 'x') === 'boolean'; });
    add(A, 'ReadableStream', 'ReadableStream', function () { var s = new ReadableStream({ start: function (c) { c.enqueue('a'); c.close(); } }); return s.getReader().read().then(function (r) { return r.value === 'a'; }); });
    add(A, 'WritableStream', 'WritableStream', function () { var got = []; var w = new WritableStream({ write: function (c) { got.push(c); } }); var wr = w.getWriter(); return wr.write('a').then(function () { return got[0] === 'a'; }); });
    add(A, 'TransformStream', 'TransformStream', function () { return typeof new TransformStream().readable.getReader === 'function'; });
    add(A, 'Response.body (stream)', 'Response', function () { return new Response('x').body.getReader().read().then(function (r) { return r.value.length === 1; }); });
    add(A, 'CompressionStream', 'CompressionStream', function () { return typeof new CompressionStream('gzip').readable === 'object'; });

    // ---------------- URL & encoding ----------------
    A = 'URL & encoding';
    add(A, 'URL', 'URL', function () { var u = new URL('/p?q=1#h', 'https://ex.test/a/'); return u.href === 'https://ex.test/p?q=1#h' && u.pathname === '/p' && u.hash === '#h'; });
    add(A, 'URL setters', 'URL', function () { var u = new URL('https://ex.test/'); u.pathname = '/z'; u.searchParams.set('a', 'b c'); return u.href === 'https://ex.test/z?a=b+c'; });
    add(A, 'URL.canParse', 'URL.canParse', function () { return URL.canParse('https://a.test') && !URL.canParse('nope'); });
    add(A, 'URL.createObjectURL', 'URL.createObjectURL', function () { return URL.createObjectURL(new Blob(['x'])).indexOf('blob:') === 0; });
    add(A, 'URL IDNA/punycode host', 'URL', function () { return new URL('https://bücher.test/').hostname === 'xn--bcher-kva.test'; });
    add(A, 'URLSearchParams', 'URLSearchParams', function () { var p = new URLSearchParams('a=1&b=2&a=3'); return p.getAll('a').join() === '1,3' && p.toString() === 'a=1&b=2&a=3'; });
    add(A, 'URLSearchParams iteration/sort', '@usp.sort', function () { var p = new URLSearchParams({ b: '1', a: '2' }); p.sort(); var k = []; for (var e of p) k.push(e[0]); return k.join() === 'a,b'; });
    add(A, 'TextEncoder', 'TextEncoder', function () { var b = new TextEncoder().encode('é'); return b.length === 2 && b[0] === 0xc3; });
    add(A, 'TextDecoder', 'TextDecoder', function () { return new TextDecoder().decode(new Uint8Array([0xe2, 0x82, 0xac])) === '€'; });
    add(A, 'TextDecoder (non-UTF-8 label)', 'TextDecoder', function () { return new TextDecoder('windows-1252').decode(new Uint8Array([0x80])) === '€'; });
    add(A, 'structuredClone', 'structuredClone', function () { var o = { a: [1], d: new Date(0), m: new Map([[1, 2]]) }; o.self = o; var c = structuredClone(o); return c !== o && c.self === c && c.m.get(1) === 2 && c.d.getTime() === 0; });

    // ---------------- Blob & files ----------------
    A = 'Blob, File & FormData';
    add(A, 'Blob', 'Blob', function () { var b = new Blob(['ab', 'c'], { type: 'text/plain' }); return b.size === 3 && b.type === 'text/plain'; });
    add(A, 'Blob.text', '@blob.text', function () { return new Blob(['hi']).text().then(function (t) { return t === 'hi'; }); });
    add(A, 'Blob.slice', '@blob.slice', function () { return new Blob(['abcd']).slice(1, 3).size === 2; });
    add(A, 'Blob.arrayBuffer', '@blob.arrayBuffer', function () { return new Blob(['abc']).arrayBuffer().then(function (b) { return b.byteLength === 3; }); });
    add(A, 'File', 'File', function () { var f = new File(['x'], 'a.txt', { type: 'text/plain' }); return f.name === 'a.txt' && f.size === 1 && f instanceof Blob; });
    add(A, 'FileReader.readAsText', 'FileReader', function () { return later(function (r) { var fr = new FileReader(); fr.onload = function () { r(fr.result === 'hi'); }; fr.readAsText(new Blob(['hi'])); }); });
    add(A, 'FileReader.readAsDataURL', 'FileReader', function () { return later(function (r) { var fr = new FileReader(); fr.onload = function () { r(fr.result === 'data:text/plain;base64,aGk='); }; fr.readAsDataURL(new Blob(['hi'], { type: 'text/plain' })); }); });
    add(A, 'FormData', 'FormData', function () { var f = new FormData(); f.append('a', '1'); f.append('a', '2'); return f.getAll('a').length === 2 && f.get('a') === '1'; });
    add(A, 'FormData(form)', 'FormData', function () { var f = new FormData(d.getElementById('f')); return f.get('q') !== null; });
    add(A, 'DataTransfer', 'DataTransfer', function () { var t = new DataTransfer(); t.setData('text/plain', 'x'); return t.getData('text/plain') === 'x'; });

    // ---------------- Storage ----------------
    A = 'Storage';
    add(A, 'localStorage', 'localStorage', function () { localStorage.setItem('k', 'v'); var ok = localStorage.getItem('k') === 'v'; localStorage.removeItem('k'); return ok && localStorage.getItem('k') === null; });
    add(A, 'localStorage.length/key/clear', 'localStorage', function () { localStorage.clear(); localStorage.setItem('a', '1'); var ok = localStorage.length === 1 && localStorage.key(0) === 'a'; localStorage.clear(); return ok; });
    add(A, 'sessionStorage', 'sessionStorage', function () { sessionStorage.setItem('k', 'v'); var ok = sessionStorage.getItem('k') === 'v'; sessionStorage.removeItem('k'); return ok; });
    add(A, 'Storage property access (localStorage.foo)', 'localStorage', function () { localStorage.setItem('pa', '1'); var ok = localStorage.pa === '1'; localStorage.removeItem('pa'); return ok; });
    add(A, 'indexedDB.open', 'indexedDB', function () { var r = indexedDB.open('census', 1); return typeof r === 'object' && r !== null; });
    add(A, 'caches (CacheStorage)', 'caches', shallow(function () { return typeof caches.open === 'function'; }));
    add(A, 'navigator.storage.estimate', 'navigator.storage', shallow(function () { return typeof navigator.storage.estimate === 'function'; }));
    add(A, 'cookieStore', 'cookieStore', shallow(function () { return typeof cookieStore.get === 'function'; }));

    // ---------------- Location / history / navigator ----------------
    A = 'Location, History & Navigator';
    add(A, 'location.href', 'location.href', function () { return location.href === 'https://census.test/start/index.html?a=1#top'; });
    add(A, 'location parts', 'location', function () { return location.protocol === 'https:' && location.host === 'census.test' && location.pathname === '/start/index.html' && location.search === '?a=1' && location.hash === '#top' && location.origin === 'https://census.test'; });
    add(A, 'location.assign/replace/reload', 'location.assign', shallow(function () { return typeof location.replace === 'function' && typeof location.reload === 'function'; }));
    add(A, 'history.length/state', 'history.state', function () { return typeof history.length === 'number' && 'state' in history; });
    add(A, 'history.pushState', 'history.pushState', function () { history.pushState({ s: 1 }, '', '/start/pushed'); return history.state.s === 1 && location.pathname === '/start/pushed'; });
    add(A, 'history.replaceState', 'history.replaceState', function () { history.replaceState({ s: 2 }, '', '/start/index.html?a=1#top'); return history.state.s === 2 && location.search === '?a=1'; });
    add(A, 'history.back/forward/go', 'history.back', shallow(function () { return typeof history.go === 'function' && typeof history.forward === 'function'; }));
    add(A, 'navigation (Navigation API)', 'navigation', shallow(function () { return typeof navigation.navigate === 'function'; }));
    add(A, 'navigator.userAgent', 'navigator.userAgent', function () { return typeof navigator.userAgent === 'string' && navigator.userAgent.length > 0; });
    add(A, 'navigator.language(s)', 'navigator.language', function () { return typeof navigator.language === 'string' && Array.isArray(navigator.languages); });
    add(A, 'navigator.platform/vendor', 'navigator.platform', function () { return typeof navigator.platform === 'string'; });
    add(A, 'navigator.onLine', 'navigator.onLine', function () { return typeof navigator.onLine === 'boolean'; });
    add(A, 'navigator.cookieEnabled', 'navigator.cookieEnabled', function () { return typeof navigator.cookieEnabled === 'boolean'; });
    add(A, 'navigator.hardwareConcurrency', 'navigator.hardwareConcurrency', function () { return navigator.hardwareConcurrency >= 1; });
    add(A, 'navigator.maxTouchPoints', 'navigator.maxTouchPoints', function () { return typeof navigator.maxTouchPoints === 'number'; });
    add(A, 'navigator.webdriver', 'navigator', function () { return typeof navigator.webdriver === 'boolean'; });
    add(A, 'navigator.userAgentData', 'navigator.userAgentData', function () { return Array.isArray(navigator.userAgentData.brands); });
    add(A, 'navigator.clipboard', 'navigator.clipboard', shallow(function () { return typeof navigator.clipboard.writeText === 'function'; }));
    add(A, 'navigator.serviceWorker', 'navigator.serviceWorker', shallow(function () { return typeof navigator.serviceWorker.register === 'function'; }));
    add(A, 'navigator.permissions.query', 'navigator.permissions', shallow(function () { return typeof navigator.permissions.query === 'function'; }));
    add(A, 'navigator.mediaDevices', 'navigator.mediaDevices', shallow(function () { return typeof navigator.mediaDevices.getUserMedia === 'function'; }));
    add(A, 'navigator.geolocation', 'navigator.geolocation', shallow(function () { return typeof navigator.geolocation.getCurrentPosition === 'function'; }));
    add(A, 'navigator.share', 'navigator.share', shallow(function () { return typeof navigator.share === 'function'; }));
    add(A, 'navigator.vibrate', 'navigator.vibrate', shallow(function () { return typeof navigator.vibrate === 'function'; }));
    add(A, 'navigator.connection', 'navigator.connection', function () { return typeof navigator.connection.effectiveType === 'string'; });

    // ---------------- CSSOM & view ----------------
    A = 'CSSOM & view';
    add(A, 'getComputedStyle', 'getComputedStyle', function () { var cs = getComputedStyle(m()); return typeof cs.getPropertyValue('display') === 'string'; });
    add(A, 'getComputedStyle reflects inline style', 'getComputedStyle', function () { var e = el('div'); d.body.appendChild(e); e.style.display = 'none'; var v = getComputedStyle(e).display; e.remove(); return v === 'none'; });
    add(A, 'matchMedia', 'matchMedia', function () { var q = matchMedia('(min-width: 1px)'); return typeof q.matches === 'boolean' && q.media === '(min-width: 1px)'; });
    add(A, 'matchMedia evaluates width', 'matchMedia', function () { return matchMedia('(min-width: 1px)').matches === true && matchMedia('(max-width: 1px)').matches === false; });
    add(A, 'MediaQueryList.addEventListener', 'MediaQueryList', function () { matchMedia('(prefers-color-scheme: dark)').addEventListener('change', function () {}); return true; });
    add(A, 'CSS.supports', 'CSS.supports', function () { return CSS.supports('display', 'grid') === true; });
    add(A, 'CSS.escape', 'CSS.escape', function () { return CSS.escape('a b') === 'a\\ b'; });
    add(A, 'document.styleSheets[i].cssRules', 'document.styleSheets', function () { return d.styleSheets.length >= 1 && d.styleSheets[0].cssRules.length >= 1; });
    add(A, 'CSSStyleSheet.insertRule', '@sheet.insertRule', function () { var s = d.styleSheets[0]; var n = s.cssRules.length; s.insertRule('.z{color:red}', n); return s.cssRules.length === n + 1; });
    add(A, 'DOMRect', 'DOMRect', function () { var r = new DOMRect(1, 2, 3, 4); return r.right === 4 && r.bottom === 6; });
    add(A, 'DOMMatrix', 'DOMMatrix', function () { return new DOMMatrix().isIdentity === true; });
    add(A, 'DOMPoint', 'DOMPoint', function () { return new DOMPoint(1, 2).y === 2; });

    // ---------------- Parsing & serialization ----------------
    A = 'Parsing & serialization';
    add(A, 'DOMParser text/html', 'DOMParser', function () { var doc = new DOMParser().parseFromString('<p id=x>hi</p>', 'text/html'); return doc.getElementById('x').textContent === 'hi'; });
    add(A, 'DOMParser image/svg+xml', 'DOMParser', function () { var doc = new DOMParser().parseFromString('<svg xmlns="http://www.w3.org/2000/svg"><g id="g"/></svg>', 'image/svg+xml'); return doc.documentElement.nodeName === 'svg'; });
    add(A, 'DOMParser application/xml', 'DOMParser', function () { var doc = new DOMParser().parseFromString('<a><b>1</b></a>', 'application/xml'); return doc.getElementsByTagName('b')[0].textContent === '1'; });
    add(A, 'XMLSerializer', 'XMLSerializer', function () { var e = el('p'); e.textContent = 'x'; return new XMLSerializer().serializeToString(e).indexOf('<p') === 0; });
    add(A, 'Range.createContextualFragment', '@range.createContextualFragment', function () { return d.createRange().createContextualFragment('<i>x</i>').firstChild.tagName === 'I'; });
    add(A, 'Element.setHTMLUnsafe', '@div.setHTMLUnsafe', function () { var e = el('div'); e.setHTMLUnsafe('<b>x</b>'); return e.firstChild.tagName === 'B'; });

    // ---------------- Range & Selection ----------------
    A = 'Range & Selection';
    add(A, 'Range (ctor)', 'Range', function () { return new Range().collapsed === true; });
    add(A, 'Range.selectNodeContents/toString', '@range.selectNodeContents', function () { var r = d.createRange(); r.selectNodeContents(d.getElementById('a1')); return r.toString() === 'One'; });
    add(A, 'Range.setStart/setEnd', '@range.setStart', function () { var t = d.getElementById('a1').firstChild; var r = d.createRange(); r.setStart(t, 1); r.setEnd(t, 3); return r.toString() === 'ne'; });
    add(A, 'Range.deleteContents', '@range.deleteContents', function () { var p = el('p'); p.textContent = 'abcd'; var r = d.createRange(); r.setStart(p.firstChild, 1); r.setEnd(p.firstChild, 3); r.deleteContents(); return p.textContent === 'ad'; });
    add(A, 'Range.extractContents/cloneContents', '@range.cloneContents', function () { var p = el('p'); p.innerHTML = '<b>x</b><i>y</i>'; var r = d.createRange(); r.selectNodeContents(p); return r.cloneContents().childNodes.length === 2; });
    add(A, 'Range.insertNode/surroundContents', '@range.insertNode', function () { var p = el('p'); p.textContent = 'ab'; var r = d.createRange(); r.setStart(p.firstChild, 1); r.collapse(true); r.insertNode(el('i')); return p.childNodes.length === 3; });
    add(A, 'Range.getBoundingClientRect', '@range.getBoundingClientRect', function () { var r = d.createRange(); r.selectNodeContents(m()); return typeof r.getBoundingClientRect().width === 'number'; });
    add(A, 'StaticRange', 'StaticRange', function () { var t = d.createTextNode('ab'); return new StaticRange({ startContainer: t, startOffset: 0, endContainer: t, endOffset: 1 }).endOffset === 1; });
    add(A, 'Selection.addRange/toString', '@sel.addRange', function () { var s = getSelection(); s.removeAllRanges(); var r = d.createRange(); r.selectNodeContents(d.getElementById('a1')); s.addRange(r); var ok = s.toString() === 'One' && s.rangeCount === 1; s.removeAllRanges(); return ok; });
    add(A, 'Selection.collapse/extend', '@sel.extend', function () { var s = getSelection(); var t = d.getElementById('a1').firstChild; s.collapse(t, 0); s.extend(t, 2); var ok = s.toString() === 'On'; s.removeAllRanges(); return ok; });

    // ---------------- Crypto & performance ----------------
    A = 'Crypto & performance';
    add(A, 'crypto.getRandomValues', 'crypto.getRandomValues', function () { var a = new Uint8Array(16); var r = crypto.getRandomValues(a); var nz = 0; for (var i = 0; i < 16; i++) if (a[i]) nz++; return r === a && nz > 0; });
    add(A, 'crypto.randomUUID', 'crypto.randomUUID', function () { return /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID()); });
    add(A, 'crypto.subtle.digest', 'crypto.subtle.digest', function () { return crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc')).then(function (b) { return new Uint8Array(b)[0] === 0xba; }); });
    add(A, 'performance.now', 'performance.now', function () { var a = performance.now(); return typeof a === 'number' && performance.now() >= a; });
    add(A, 'performance.timeOrigin', 'performance.timeOrigin', function () { return performance.timeOrigin > 0; });
    add(A, 'performance.mark/measure', 'performance.mark', function () { performance.mark('a'); performance.mark('b'); performance.measure('ab', 'a', 'b'); return performance.getEntriesByName('ab').length === 1; });
    add(A, 'performance.getEntriesByType', 'performance.getEntriesByType', function () { return Array.isArray(performance.getEntriesByType('navigation')); });
    add(A, 'performance.timing (legacy)', 'performance.timing', function () { return typeof performance.timing.navigationStart === 'number'; });
    add(A, 'Date.now', 'Date.now', function () { return Date.now() > 1600000000000; });

    // ---------------- Console ----------------
    A = 'Console';
    add(A, 'console.log', 'console.log', function () { console.log('census'); return true; });
    add(A, 'console.error/warn/info/debug', 'console.warn', function () { console.warn('w'); console.error('e'); console.info('i'); console.debug('d'); return true; });
    add(A, 'console.table/dir', 'console.table', function () { console.table([{ a: 1 }]); console.dir({}); return true; });
    add(A, 'console.group/groupEnd', 'console.group', function () { console.group('g'); console.groupEnd(); return true; });
    add(A, 'console.time/timeEnd', 'console.time', function () { console.time('t'); console.timeEnd('t'); return true; });
    add(A, 'console.assert/count/trace', 'console.assert', function () { console.assert(true); console.count('c'); console.trace(); return true; });

    // ---------------- Workers & messaging ----------------
    A = 'Workers & messaging';
    add(A, 'Worker', 'Worker', shallow(function () { return typeof Worker === 'function'; }));
    add(A, 'SharedWorker', 'SharedWorker', shallow(function () { return typeof SharedWorker === 'function'; }));
    add(A, 'BroadcastChannel', 'BroadcastChannel', function () { var c = new BroadcastChannel('census'); c.close(); return true; });
    add(A, 'window message event via postMessage', 'postMessage', function () { return later(function (r) { addEventListener('message', function h(e) { if (e.data === 'census-pm') { removeEventListener('message', h); r(true); } }); postMessage('census-pm', '*'); }); });

    // ---------------- Graphics & media (presence only beyond basic) ----------------
    A = 'Graphics & media';
    add(A, 'CanvasRenderingContext2D', 'CanvasRenderingContext2D', function () { return el('canvas').getContext('2d') instanceof CanvasRenderingContext2D; });
    add(A, 'OffscreenCanvas', 'OffscreenCanvas', function () { return new OffscreenCanvas(1, 1).getContext('2d') !== null; });
    add(A, 'ImageData', 'ImageData', function () { return new ImageData(2, 2).data.length === 16; });
    add(A, 'Path2D', 'Path2D', function () { var p = new Path2D(); p.rect(0, 0, 1, 1); return true; });
    add(A, 'createImageBitmap', 'createImageBitmap', shallow(function () { return typeof createImageBitmap === 'function'; }));
    add(A, 'WebGLRenderingContext', 'WebGLRenderingContext', shallow(function () { return typeof WebGLRenderingContext === 'function'; }));
    add(A, 'canvas.getContext("webgl")', '@canvas.getContext', function () { return el('canvas').getContext('webgl') !== null; });
    add(A, 'AudioContext', 'AudioContext', shallow(function () { return typeof AudioContext === 'function'; }));
    add(A, 'MediaSource', 'MediaSource', shallow(function () { return typeof MediaSource === 'function'; }));
    add(A, 'Notification', 'Notification', function () { return typeof Notification.permission === 'string'; });
    add(A, 'speechSynthesis', 'speechSynthesis', shallow(function () { return typeof speechSynthesis.speak === 'function'; }));
    add(A, 'FontFace', 'FontFace', function () { return new FontFace('x', 'url(x.woff2)').family === 'x'; });

    // ---------------- Added 2026-10-05 ----------------
    // Members referenced in src/web_*.js that the 2026-10-03 probe did not
    // cover. Each goes in the area it belongs to; the source file is noted.

    // web_platform.js
    A = 'Window & globals';
    add(A, 'window.outerWidth/outerHeight', 'outerWidth', function () { return outerWidth > 0 && outerHeight > 0; });
    add(A, 'window.opener/closed', 'closed', function () { return opener === null && closed === false; });
    add(A, 'screen.availWidth/colorDepth', 'screen.availWidth', function () { return screen.availWidth > 0 && screen.colorDepth > 0; });
    add(A, 'screen.orientation', 'screen.orientation', function () { return typeof screen.orientation.type === 'string'; });
    add(A, 'window.scroll (alias)', 'scroll', function () { scroll(0, 0); return scrollY === 0; });
    // web_encoding.js
    add(A, 'escape/unescape', 'escape', function () { return escape('a b') === 'a%20b' && unescape('a%20b') === 'a b'; });
    // web_interfaces.js
    add(A, 'DOMException', 'DOMException', function () { var e = new DOMException('m', 'AbortError'); return e.name === 'AbortError' && e.code === 20; });
    add(A, 'Window/Navigator/Location/History/Screen interfaces', 'Navigator', function () { return window instanceof Window && navigator instanceof Navigator && location instanceof Location && history instanceof History && screen instanceof Screen; });

    // web_intl.js
    A = 'Intl';
    add(A, 'Intl.NumberFormat currency', 'Intl.NumberFormat', function () { return new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).format(1234.5) === '$1,234.50'; });
    add(A, 'Intl.NumberFormat compact', 'Intl.NumberFormat', function () { return new Intl.NumberFormat('en-US', { notation: 'compact' }).format(1500) === '1.5K'; });
    add(A, 'Intl.NumberFormat.formatToParts', 'Intl.NumberFormat.prototype.formatToParts', function () { return new Intl.NumberFormat('en-US').formatToParts(1234)[1].type === 'group'; });
    add(A, 'Intl.DateTimeFormat options', 'Intl.DateTimeFormat', function () { return new Intl.DateTimeFormat('en-US', { timeZone: 'UTC', year: 'numeric', month: 'long', day: 'numeric' }).format(new Date(0)) === 'January 1, 1970'; });
    add(A, 'Intl.DateTimeFormat.formatToParts', 'Intl.DateTimeFormat.prototype.formatToParts', function () { return new Intl.DateTimeFormat('en-US', { timeZone: 'UTC' }).formatToParts(new Date(0)).some(function (p) { return p.type === 'year' && p.value === '1970'; }); });
    add(A, 'Intl.DateTimeFormat resolvedOptions().timeZone', 'Intl.DateTimeFormat', function () { return typeof new Intl.DateTimeFormat().resolvedOptions().timeZone === 'string'; });
    add(A, 'Intl.DisplayNames', 'Intl.DisplayNames', function () { return new Intl.DisplayNames('en', { type: 'region' }).of('US') === 'United States'; });
    add(A, 'Intl.supportedValuesOf', 'Intl.supportedValuesOf', function () { return Intl.supportedValuesOf('currency').indexOf('USD') >= 0; });
    add(A, 'Date.prototype.toLocaleTimeString', 'Date.prototype.toLocaleTimeString', function () { return new Date(0).toLocaleTimeString('en-US', { timeZone: 'UTC' }) === '12:00:00 AM'; });
    add(A, 'Date.prototype.toLocaleString (en-US, UTC)', 'Date.prototype.toLocaleString', function () { return new Date(0).toLocaleString('en-US', { timeZone: 'UTC' }) === '1/1/1970, 12:00:00 AM'; });

    // web_document.js
    A = 'Document';
    add(A, 'document.anchors/embeds', 'document.anchors', function () { return typeof d.anchors.length === 'number' && typeof d.embeds.length === 'number'; });
    add(A, 'document.fonts.ready resolves', 'document.fonts', function () { return d.fonts.ready.then(function (f) { return f === d.fonts; }); });
    add(A, 'document.fonts.check/load', 'document.fonts.check', function () { return typeof d.fonts.check('12px x') === 'boolean' && typeof d.fonts.load === 'function'; });
    add(A, 'document.fullscreenEnabled', 'document.fullscreenEnabled', function () { return typeof d.fullscreenEnabled === 'boolean'; });
    // web_scroll.js
    add(A, 'document.scrollingElement', 'document.scrollingElement', function () { return d.scrollingElement === d.documentElement; });
    // web_interfaces.js
    add(A, 'document.implementation (DOMImplementation)', 'document.implementation', function () { return d.implementation instanceof DOMImplementation; });

    // web_scroll.js
    A = 'Element';
    add(A, 'Element.scrollLeft/scrollBy', '@div.scrollBy', function () { var e = el('div'); e.scrollBy(0, 0); return typeof e.scrollLeft === 'number'; });
    // web_shadow.js
    add(A, 'slot.assignedElements / assignedSlot', '@div.attachShadow', function () { var h = el('div'); var c = h.appendChild(el('i')); var s = h.attachShadow({ mode: 'open' }); s.innerHTML = '<slot></slot>'; return s.firstChild.assignedElements().length === 1 && c.assignedSlot === s.firstChild; });
    add(A, 'closed shadow root hides shadowRoot', '@div.attachShadow', function () { var h = el('div'); h.attachShadow({ mode: 'closed' }); return h.shadowRoot === null; });
    add(A, 'event retargeting across shadow boundary', '@div.attachShadow', function () { var h = el('div'); var s = h.attachShadow({ mode: 'open' }); s.innerHTML = '<b></b>'; var t; h.addEventListener('x', function (e) { t = e.target; }); s.firstChild.dispatchEvent(new Event('x', { bubbles: true, composed: true })); return t === h; });

    // web_forms.js
    A = 'HTML elements';
    add(A, 'option.selected/defaultSelected', '@select.options', function () { var s = d.getElementById('sel'); return s.options[1].selected === true && s.options[1].defaultSelected === true; });
    add(A, 'input.defaultChecked/indeterminate', '@input.indeterminate', function () { var i = el('input'); i.type = 'checkbox'; i.indeterminate = true; return i.indeterminate === true && i.defaultChecked === false; });
    add(A, 'setCustomValidity/reportValidity', '@input.setCustomValidity', function () { var i = el('input'); i.setCustomValidity('bad'); return i.validity.customError === true && i.validationMessage === 'bad' && typeof i.reportValidity === 'function'; });
    add(A, 'fieldset.elements/disabled', 'HTMLFieldSetElement', function () { var f = el('fieldset'); f.appendChild(el('input')); return f.elements.length === 1 && f.disabled === false; });
    add(A, 'form.length/namedItem', '@form.length', function () { var f = d.getElementById('f'); return f.length >= 2 && f.elements.namedItem('q') === d.getElementById('q'); });
    add(A, 'SubmitEvent.submitter', 'SubmitEvent', function () { var b = el('button'); return new SubmitEvent('submit', { submitter: b }).submitter === b; });
    // web_history.js
    add(A, 'HTMLAnchorElement.origin', '@a.origin', function () { return d.getElementById('lnk').origin === 'https://census.test'; });
    // web_document.js / web_interfaces.js
    add(A, 'HTMLDetailsElement / details toggle', 'HTMLDetailsElement', function () { var x = el('details'); x.open = true; return x instanceof HTMLDetailsElement && x.hasAttribute('open'); });

    // web_interfaces.js
    A = 'Events';
    add(A, 'UIEvent / CompositionEvent', 'CompositionEvent', function () { return new CompositionEvent('compositionend', { data: 'x' }).data === 'x' && new UIEvent('x') instanceof Event; });
    add(A, 'HashChangeEvent / PopStateEvent / PageTransitionEvent', 'HashChangeEvent', function () { return new HashChangeEvent('hashchange', { newURL: 'u' }).newURL === 'u' && new PopStateEvent('popstate', { state: 1 }).state === 1 && new PageTransitionEvent('pageshow', { persisted: true }).persisted === true; });
    add(A, 'StorageEvent / ProgressEvent', 'StorageEvent', function () { return new StorageEvent('storage', { key: 'k' }).key === 'k' && new ProgressEvent('progress', { loaded: 3 }).loaded === 3; });
    add(A, 'KeyboardEvent.getModifierState', 'KeyboardEvent.prototype.getModifierState', function () { return new KeyboardEvent('keydown', { shiftKey: true }).getModifierState('Shift') === true; });
    add(A, 'MediaQueryListEvent', 'MediaQueryListEvent', function () { return new MediaQueryListEvent('change', { matches: true }).matches === true; });

    // web_observers_live.js
    A = 'Observers';
    add(A, 'IntersectionObserver rootMargin/thresholds', 'IntersectionObserver', function () { var o = new IntersectionObserver(function () {}, { rootMargin: '10px', threshold: [0, 0.5] }); return o.thresholds.length === 2 && typeof o.rootMargin === 'string'; });
    add(A, 'IntersectionObserver.takeRecords', 'IntersectionObserver.prototype.takeRecords', function () { var o = new IntersectionObserver(function () {}); return Array.isArray(o.takeRecords()); });
    add(A, 'IntersectionObserverEntry / ResizeObserverEntry / MutationRecord', 'IntersectionObserverEntry', function () { return typeof IntersectionObserverEntry === 'function' && typeof ResizeObserverEntry === 'function' && typeof MutationRecord === 'function'; });

    // web_components.js
    A = 'Web Components';
    add(A, 'customElements.getName', 'customElements.getName', function () { var C = class extends HTMLElement {}; customElements.define('census-getname', C); return customElements.getName(C) === 'census-getname'; });
    add(A, 'connected/disconnectedCallback', 'customElements', function () { var log = []; customElements.define('census-life', class extends HTMLElement { connectedCallback() { log.push('c'); } disconnectedCallback() { log.push('d'); } }); var x = el('census-life'); d.body.appendChild(x); x.remove(); return log.join('') === 'cd'; });
    add(A, 'CustomElementRegistry (interface)', 'CustomElementRegistry', function () { return customElements instanceof CustomElementRegistry; });

    // web_xhr.js / web_fetch.js / web_streams.js
    A = 'Networking';
    add(A, 'XMLHttpRequest.upload / getAllResponseHeaders', 'XMLHttpRequestUpload', function () { var x = new XMLHttpRequest(); return x.upload instanceof XMLHttpRequestUpload && x.getAllResponseHeaders() === ''; });
    add(A, 'XMLHttpRequest synchronous open', 'XMLHttpRequest', function () { var x = new XMLHttpRequest(); x.open('GET', '/x', false); return true; });
    add(A, 'Request.clone / Response.clone', 'Response.prototype.clone', function () { var r = new Response('ab'); return r.clone().text().then(function (t) { return t === 'ab' && new Request('/x').clone().url === 'https://census.test/x'; }); });
    add(A, 'Response.error / Response.redirect', 'Response.redirect', function () { return Response.error().type === 'error' && Response.redirect('https://census.test/r', 302).status === 302; });
    add(A, 'Response.formData (urlencoded)', 'Response.prototype.formData', function () { return new Response('a=1', { headers: { 'content-type': 'application/x-www-form-urlencoded' } }).formData().then(function (f) { return f.get('a') === '1'; }); });
    add(A, 'ReadableStream.tee / pipeThrough', 'ReadableStream.prototype.tee', function () { var rs = new ReadableStream({ start: function (c) { c.enqueue('x'); c.close(); } }); var t = rs.tee(); return t[0].pipeThrough(new TransformStream()).getReader().read().then(function (r) { return r.value === 'x'; }); });
    add(A, 'ReadableStream async iteration', 'ReadableStream', function () { var rs = new ReadableStream({ start: function (c) { c.enqueue(1); c.enqueue(2); c.close(); } }); return (0, eval)('(async function(rs){ var s = 0; for await (var v of rs) s += v; return s; })')(rs).then(function (s) { return s === 3; }); });
    add(A, 'CountQueuingStrategy / ByteLengthQueuingStrategy', 'CountQueuingStrategy', function () { return new CountQueuingStrategy({ highWaterMark: 2 }).highWaterMark === 2 && new ByteLengthQueuingStrategy({ highWaterMark: 8 }).size(new Uint8Array(3)) === 3; });
    add(A, 'TextEncoderStream / TextDecoderStream', 'TextEncoderStream', function () { return new TextEncoderStream().encoding === 'utf-8' && new TextDecoderStream().encoding === 'utf-8'; });
    add(A, 'byob reader', 'ReadableStream', function () { new ReadableStream({ type: 'bytes' }).getReader({ mode: 'byob' }); return true; });

    // web_url.js / web_encoding.js
    A = 'URL & encoding';
    add(A, 'URL.revokeObjectURL', 'URL.revokeObjectURL', function () { URL.revokeObjectURL(URL.createObjectURL(new Blob(['x']))); return true; });
    add(A, 'URL.parse', 'URL.parse', function () { return URL.parse('nope') === null && URL.parse('https://a.test/').host === 'a.test'; });
    add(A, 'webkitURL alias', 'webkitURL', function () { return webkitURL === URL; });
    add(A, 'URLSearchParams.size', 'URLSearchParams', function () { return new URLSearchParams('a=1&b=2').size === 2; });
    add(A, 'TextEncoder.encodeInto', 'TextEncoder.prototype.encodeInto', function () { var u = new Uint8Array(4); var r = new TextEncoder().encodeInto('hi', u); return r.written === 2 && u[0] === 104; });
    add(A, 'TextDecoder fatal', 'TextDecoder', function () { try { new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array([0xff])); return false; } catch (e) { return e instanceof TypeError; } });

    // web_blob.js
    A = 'Blob, File & FormData';
    add(A, 'Blob.bytes', 'Blob.prototype.bytes', function () { return new Blob(['ab']).bytes().then(function (u) { return u instanceof Uint8Array && u.length === 2; }); });
    add(A, 'File.lastModified/name', 'File', function () { var f = new File(['x'], 'n.txt', { lastModified: 5 }); return f.name === 'n.txt' && f.lastModified === 5; });
    add(A, 'FormData.entries/getAll', 'FormData.prototype.getAll', function () { var f = new FormData(); f.append('a', '1'); f.append('a', '2'); return f.getAll('a').length === 2 && Array.from(f.entries()).length === 2; });
    add(A, 'AbortSignal.abort / throwIfAborted', 'AbortSignal.abort', function () { var s = AbortSignal.abort(); try { s.throwIfAborted(); return false; } catch (e) { return s.aborted && e.name === 'AbortError'; } });

    // web_history.js
    A = 'Location, History & Navigator';
    add(A, 'history.scrollRestoration', 'history.scrollRestoration', function () { return history.scrollRestoration === 'auto'; });
    add(A, 'popstate on history.back()', 'history.back', function () { var u = location.href; history.pushState({ p: 1 }, '', '/start/pop'); return later(function (r) { addEventListener('popstate', function f() { removeEventListener('popstate', f); r(location.href === u); }); history.back(); }); });
    add(A, 'pushState cross-origin throws SecurityError', 'history.pushState', function () { try { history.pushState(null, '', 'https://other.test/'); return false; } catch (e) { return e.name === 'SecurityError'; } });
    // web_platform.js
    add(A, 'navigator.plugins/mimeTypes/pdfViewerEnabled', 'navigator.plugins', function () { return typeof navigator.plugins.length === 'number' && typeof navigator.mimeTypes.length === 'number' && typeof navigator.pdfViewerEnabled === 'boolean'; });
    add(A, 'navigator.doNotTrack/javaEnabled', 'navigator.javaEnabled', function () { return navigator.javaEnabled() === false && 'doNotTrack' in navigator; });

    // web_cssom.js / web_dommatrix.js
    A = 'CSSOM & view';
    add(A, 'CSSStyleSheet.replaceSync + adoptedStyleSheets', 'CSSStyleSheet.prototype.replaceSync', function () { var s = new CSSStyleSheet(); s.replaceSync('a{color:red}'); d.adoptedStyleSheets = [s]; return s.cssRules.length === 1 && d.adoptedStyleSheets[0] === s; });
    add(A, 'CSSStyleSheet.replace (Promise)', 'CSSStyleSheet.prototype.replace', function () { return new CSSStyleSheet().replace('b{color:red}').then(function (s) { return s.cssRules.length === 1; }); });
    add(A, 'CSSStyleSheet.deleteRule', 'CSSStyleSheet.prototype.deleteRule', function () { var s = new CSSStyleSheet(); s.replaceSync('a{color:red} b{color:blue}'); s.deleteRule(0); return s.cssRules.length === 1; });
    add(A, 'CSSStyleRule.selectorText / style.setProperty', 'CSSStyleRule', function () { var s = new CSSStyleSheet(); s.replaceSync('a{color:red}'); var r = s.cssRules[0]; r.style.setProperty('color', 'blue'); return r instanceof CSSStyleRule && r.selectorText === 'a' && r.style.getPropertyValue('color') === 'blue'; });
    add(A, '<style>.sheet reflects text', '@style.sheet', function () { var st = el('style'); st.textContent = 'p{margin:0}'; d.head.appendChild(st); var ok = st.sheet.cssRules.length === 1; st.remove(); return ok; });
    add(A, 'DOMMatrix from CSS string', 'DOMMatrix', function () { var m = new DOMMatrix('translate(10px, 20px) scale(2)'); return m.e === 10 && m.f === 20 && m.a === 2; });
    add(A, 'DOMMatrix.multiply/inverse', 'DOMMatrix.prototype.multiply', function () { var m = new DOMMatrix([2, 0, 0, 2, 5, 5]); return m.multiply(m.inverse()).isIdentity === true; });
    add(A, 'DOMMatrixReadOnly / WebKitCSSMatrix', 'DOMMatrixReadOnly', function () { return typeof DOMMatrixReadOnly === 'function' && typeof WebKitCSSMatrix === 'function'; });
    add(A, 'DOMRectReadOnly / DOMPointReadOnly', 'DOMRectReadOnly', function () { return new DOMRectReadOnly(1, 2, 3, 4).right === 4 && new DOMPointReadOnly(1, 2).y === 2; });

    // web_crypto.js
    A = 'Crypto & performance';
    add(A, 'crypto.getRandomValues rejects Float32Array', 'crypto.getRandomValues', function () { try { crypto.getRandomValues(new Float32Array(1)); return false; } catch (e) { return e.name === 'TypeMismatchError'; } });
    // web_platform.js
    add(A, 'performance.getEntriesByName / clearMarks', 'performance.getEntriesByName', function () { performance.mark('census-m'); var n = performance.getEntriesByName('census-m').length; performance.clearMarks('census-m'); return n === 1 && performance.getEntriesByName('census-m').length === 0; });
    add(A, 'performance.toJSON', 'performance.toJSON', function () { return typeof performance.toJSON().timeOrigin === 'number'; });

    // ---------------- run ----------------
    var results = [];
    var pending = [];
    for (var i = 0; i < E.length; i++) {
        var e = E[i];
        var rec = { area: e[0], name: e[1], path: e[2], status: '', detail: '', shallow: e[3].shallow === true };
        results.push(rec);
        var v;
        try { v = resolve(e[2]); } catch (err) { v = undefined; rec.detail = 'instance unavailable: ' + (err && err.message); }
        if (v === undefined) { rec.status = 'missing'; continue; }
        try {
            var out = e[3]();
            if (out && typeof out.then === 'function') {
                rec.status = 'pending';
                (function (rec) {
                    out.then(function (ok) {
                        rec.status = ok === true ? 'works' : 'broken';
                        if (ok !== true) rec.detail = 'async result check failed (got ' + String(ok) + ')';
                    }, function (err) {
                        rec.status = 'broken';
                        rec.detail = 'rejected: ' + (err && err.name ? err.name + ': ' + err.message : String(err));
                    });
                })(rec);
            } else if (out === true) {
                rec.status = 'works';
            } else {
                rec.status = 'broken';
                rec.detail = 'result check failed (got ' + String(out) + ')';
            }
        } catch (err) {
            rec.status = 'broken';
            rec.detail = 'threw: ' + (err && err.name ? err.name + ': ' + err.message : String(err));
        }
    }
    window.__census = results;
})();
