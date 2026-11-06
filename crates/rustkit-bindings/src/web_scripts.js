// Script elements that script inserts (HTML §4.12.1.1 "prepare the script
// element"): `document.createElement('script')`, a `src` or text, and an
// insert into the document. Pages load whole bundles this way (loaders,
// analytics, a challenge page's check script); until 2026-10-16 such an
// element sat in the tree and nothing fetched or ran it.
//
// An external classic script is queued for the engine, which fetches it
// like a parser-inserted one (its own request, never through page script)
// and runs it as the element's script, then fires `load` or `error`. It is
// force-async (HTML: the flag starts true; the parser is what clears it),
// so it runs when its own fetch finishes. `async = false` clears the flag
// and puts it in the in-order list. An external module is `import()`ed. An
// inline classic script runs at once.
//
// Only elements script created are prepared (`createElement`, an HTML
// `createElementNS`, or a copy of one not yet started): the parser's scripts
// ran (or were skipped) with the load, and `innerHTML` never runs scripts.
// Each element is prepared once. Not here (stated): inline module scripts and
// `document.write`.
(function (g) {
    var internals = g.__rkScriptInternals;
    delete g.__rkScriptInternals;
    var HOOKS = g.__rkDomHooks;
    if (!internals || !HOOKS) return;
    // Captured before page script can replace them.
    var indirectEval = g.eval, resolved = Promise.resolve(), URL_ = g.URL, Event_ = g.Event;
    var STATE = Symbol('rustkit.script');
    // HTML §4.12.1 "force async": an element script made is async until its
    // `async` is set or its `async` attribute changes; the parser's are not.
    var FORCE = Symbol('rustkit.forceAsync');
    var XHTML = 'http://www.w3.org/1999/xhtml';
    // Created and not yet started: inserts look for scripts only while
    // there is one.
    var fresh = 0;
    var JS_TYPE = /^(?:(?:text|application)\/(?:x-)?(?:java|ecma)script|text\/javascript1\.[0-5]|text\/(?:jscript|livescript))$/;

    function isScript(n) {
        return n != null && n.nodeType === 1 && n.localName === 'script';
    }
    // The script's type (HTML §4.12.1.1 steps 8-10): 'classic', 'module',
    // or null for a data block.
    function kind(el) {
        var type = el.getAttribute('type'), lang = el.getAttribute('language'), t;
        if (type === null ? !lang : type === '') t = 'text/javascript';
        else if (type === null) t = 'text/' + lang;
        else t = type;
        t = t.trim().toLowerCase();
        if (JS_TYPE.test(t)) return 'classic';
        return t === 'module' ? 'module' : null;
    }
    function fire(el, type) {
        resolved.then(function () { el.dispatchEvent(new Event_(type)); });
    }
    function prepare(el) {
        if (el[STATE] !== 'fresh' || !el.isConnected) return;
        var src = el.getAttribute('src');
        if (src === null && el.textContent === '') return;
        var k = kind(el);
        if (!k) return;
        el[STATE] = 'started';
        fresh--;
        if (k === 'classic' && el.hasAttribute('nomodule')) return;
        if (src !== null) {
            var url;
            try { url = new URL_(src, g.document.baseURI || g.location.href).href; } catch (e) { url = null; }
            if (!url || src === '') return fire(el, 'error');
            if (k === 'module') {
                import(url).then(function () { fire(el, 'load'); }, function () { fire(el, 'error'); });
                return;
            }
            internals.fetch(el, url, !(el[FORCE] === true || el.hasAttribute('async')));
            return;
        }
        if (k === 'module') return;
        var was = internals.current(el);
        try {
            indirectEval(el.textContent);
        } catch (e) {
            try { __rk.reportError(e); } catch (_) {}
        } finally {
            internals.current(was);
        }
    }

    // `async` reads true while the element is forced async, and setting it
    // either way ends that (web_reflect.js defined the attribute's half).
    var SP = g.HTMLScriptElement && g.HTMLScriptElement.prototype;
    var asyncIdl = SP && Object.getOwnPropertyDescriptor(SP, 'async');
    if (asyncIdl && asyncIdl.get && asyncIdl.set) {
        Object.defineProperty(SP, 'async', {
            get: function () { return this[FORCE] === true || asyncIdl.get.call(this); },
            set: function (v) { if (this[FORCE] === true) this[FORCE] = false; asyncIdl.set.call(this, v); },
            enumerable: asyncIdl.enumerable,
            configurable: true
        });
    }

    HOOKS.scripts = {
        created: function (el) {
            if (!isScript(el) || (el.namespaceURI != null && el.namespaceURI !== XHTML)) return;
            Object.defineProperty(el, STATE, { value: 'fresh', writable: true, configurable: true });
            Object.defineProperty(el, FORCE, { value: true, writable: true, configurable: true });
            fresh++;
        },
        // HTML §4.12.1 cloning steps: a copy is started when its original
        // is, so a copy of a script not yet started (or of a subtree holding
        // one) is prepared when it goes in. The parser's scripts, and those
        // `innerHTML` made, are started: their copies never run.
        cloned: function (from, copy) {
            if (fresh === 0 || from == null || copy == null) return;
            if (isScript(from)) {
                if (from[STATE] === 'fresh') this.created(copy);
                return;
            }
            if (typeof from.querySelectorAll !== 'function') return;
            var a = from.querySelectorAll('script'), b = copy.querySelectorAll('script');
            for (var i = 0; i < a.length && i < b.length; i++) {
                if (a[i][STATE] === 'fresh') this.created(b[i]);
            }
        },
        // Before `node` goes into `parent`: the fresh scripts it carries (a
        // fragment empties as it goes in), prepared in tree order after.
        // Walked, not queried: a fragment's scripts are not in the document
        // the selector walks.
        inserting: function (node, parent) {
            if (fresh === 0) return null;
            var found = [];
            function walk(n) {
                if (!n || n.localName === 'template') return;
                if (isScript(n)) found.push(n);
                for (var c = n.firstChild; c; c = c.nextSibling) walk(c);
            }
            if (isScript(node)) found.push(node);
            else walk(node);
            // Text added to a script already in the document.
            if (isScript(parent)) found.push(parent);
            found = found.filter(function (s) { return s[STATE] === 'fresh'; });
            return found.length ? function () { found.forEach(prepare); } : null;
        },
        // A `src` set, or text written, on a script already in the document.
        changed: function (node, op, name) {
            if (isScript(node) && op === 'setAttr' && String(name).toLowerCase() === 'async') node[FORCE] = false;
            if (fresh === 0 || !isScript(node)) return;
            if (op === 'setText' || (op === 'setAttr' && String(name).toLowerCase() === 'src')) prepare(node);
        }
    };
})(globalThis);
