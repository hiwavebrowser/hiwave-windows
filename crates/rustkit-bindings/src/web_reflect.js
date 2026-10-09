// HTML IDL attribute reflection (HTML §2.6.1 and the per-element sections):
// the properties pages read and write on <link>, <base>, <meta>, <script>,
// <img>, <a>/<area>, <iframe>, <form>, <style> and on every HTMLElement
// (title, lang, dir, hidden, tabIndex, ...), each backed by a content
// attribute, plus `Node.baseURI` and `document.baseURI`.
//
// Why: `link.href` was `undefined`, so github's favicon updater did
// `e.href.indexOf("-dark.svg")` and its whole module threw "cannot convert
// 'null' or 'undefined' to object" at load; `script.type`, `img.alt`,
// `a.target`, `el.tabIndex` and the rest read as `undefined` the same way.
// Each name is defined only when the element's prototype does not already
// have it (web_forms.js and the DOM layer own the form controls, `a.href`
// and `script.src`).
//
// Honest limits: `img.complete` is true only for an image with no `src`, and
// `naturalWidth/Height` are 0 (the engine does not publish decoded sizes);
// `iframe.contentWindow/contentDocument` are null; `link.sheet` stays as the
// CSSOM layer has it.
(function (g) {
    var document = g.document;
    if (!document || !g.HTMLElement) return;

    function has(proto, name) { return Object.prototype.hasOwnProperty.call(proto, name) || proto[name] !== undefined; }
    function def(proto, name, get, set) {
        if (!proto || has(proto, name)) return;
        Object.defineProperty(proto, name, { get: get, set: set, configurable: true, enumerable: true });
    }

    // ---- base URL: the document's, or its first <base href>.
    function baseURI() {
        var url = String(g.location && g.location.href || '');
        var base = document.getElementsByTagName('base');
        for (var i = 0; i < base.length; i++) {
            var raw = base[i].getAttribute('href');
            if (raw !== null) {
                try { return new g.URL(raw, url).href; } catch (_) { break; }
            }
        }
        return url;
    }
    def(g.Node.prototype, 'baseURI', baseURI);
    function resolve(raw) {
        try { return new g.URL(raw, baseURI()).href; } catch (_) { return raw; }
    }

    // ---- the reflecting forms
    function str(proto, prop, attr, dflt) {
        def(proto, prop, function () { var v = this.getAttribute(attr); return v === null ? (dflt || '') : v; },
            function (v) { this.setAttribute(attr, String(v)); });
    }
    function url(proto, prop, attr) {
        def(proto, prop, function () { var v = this.getAttribute(attr); return v === null ? '' : resolve(v); },
            function (v) { this.setAttribute(attr, String(v)); });
    }
    function bool(proto, prop, attr) {
        def(proto, prop, function () { return this.hasAttribute(attr); },
            function (v) { if (v) this.setAttribute(attr, ''); else this.removeAttribute(attr); });
    }
    function long(proto, prop, attr, dflt) {
        def(proto, prop, function () { var n = parseInt(this.getAttribute(attr), 10); return isNaN(n) ? dflt : n; },
            function (v) { this.setAttribute(attr, String(Math.trunc(+v) || 0)); });
    }
    // An enumerated attribute: the keyword when valid, else the default.
    function enm(proto, prop, attr, keywords, dflt, missing) {
        def(proto, prop, function () {
            var v = this.getAttribute(attr);
            if (v === null) return missing === undefined ? dflt : missing;
            v = v.toLowerCase();
            return keywords.indexOf(v) >= 0 ? v : dflt;
        }, function (v) { this.setAttribute(attr, String(v)); });
    }
    // crossOrigin: null when absent, "anonymous" for anything unknown.
    function cors(proto) {
        def(proto, 'crossOrigin', function () {
            var v = this.getAttribute('crossorigin');
            if (v === null) return null;
            v = v.toLowerCase();
            return v === 'use-credentials' ? v : 'anonymous';
        }, function (v) { if (v === null) this.removeAttribute('crossorigin'); else this.setAttribute('crossorigin', String(v)); });
    }

    // ---- DOMTokenList over an attribute (relList, sandbox, ...).
    function TokenList(el, attr) {
        Object.defineProperty(this, '_el', { value: el });
        Object.defineProperty(this, '_attr', { value: attr });
    }
    if (g.DOMTokenList) Object.setPrototypeOf(TokenList.prototype, g.DOMTokenList.prototype);
    function tokens(l) {
        var v = l._el.getAttribute(l._attr);
        return v === null ? [] : v.split(/\s+/).filter(function (t, i, a) { return t && a.indexOf(t) === i; });
    }
    function write(l, list) { l._el.setAttribute(l._attr, list.join(' ')); }
    function check(t, m) {
        t = String(t);
        if (t === '') throw new g.DOMException("Failed to execute '" + m + "' on 'DOMTokenList': The token provided must not be empty.", 'SyntaxError');
        if (/\s/.test(t)) throw new g.DOMException("Failed to execute '" + m + "' on 'DOMTokenList': The token provided ('" + t + "') contains HTML space characters, which are not valid in tokens.", 'InvalidCharacterError');
        return t;
    }
    var TP = TokenList.prototype;
    function tm(name, fn) { Object.defineProperty(TP, name, { value: fn, writable: true, configurable: true, enumerable: true }); }
    Object.defineProperty(TP, 'length', { get: function () { return tokens(this).length; }, configurable: true, enumerable: true });
    Object.defineProperty(TP, 'value', {
        get: function () { var v = this._el.getAttribute(this._attr); return v === null ? '' : v; },
        set: function (v) { this._el.setAttribute(this._attr, String(v)); }, configurable: true, enumerable: true
    });
    tm('item', function item(i) { return tokens(this)[i >>> 0] || null; });
    tm('contains', function contains(t) { return tokens(this).indexOf(String(t)) >= 0; });
    tm('add', function add() {
        var list = tokens(this);
        for (var i = 0; i < arguments.length; i++) { var t = check(arguments[i], 'add'); if (list.indexOf(t) < 0) list.push(t); }
        write(this, list);
    });
    tm('remove', function remove() {
        var list = tokens(this);
        for (var i = 0; i < arguments.length; i++) { var t = check(arguments[i], 'remove'); var j = list.indexOf(t); if (j >= 0) list.splice(j, 1); }
        write(this, list);
    });
    tm('toggle', function toggle(t, force) {
        t = check(t, 'toggle');
        var list = tokens(this), j = list.indexOf(t);
        if (j >= 0) { if (force === true) return true; list.splice(j, 1); write(this, list); return false; }
        if (force === false) return false;
        list.push(t); write(this, list); return true;
    });
    tm('replace', function replace(a, b) {
        a = check(a, 'replace'); b = check(b, 'replace');
        var list = tokens(this), j = list.indexOf(a);
        if (j < 0) return false;
        var k = list.indexOf(b);
        if (k >= 0 && k !== j) { list.splice(j, 1); } else list[j] = b;
        write(this, list);
        return true;
    });
    tm('supports', function supports() { return true; });
    tm('toString', function toString() { return this.value; });
    tm('forEach', function forEach(cb, thisArg) { tokens(this).forEach(function (t, i) { cb.call(thisArg, t, i, this); }, this); });
    tm('keys', function keys() { return tokens(this).keys(); });
    tm('values', function values() { return tokens(this)[Symbol.iterator](); });
    tm('entries', function entries() { return tokens(this).entries(); });
    TP[Symbol.iterator] = TP.values;
    function list(proto, prop, attr) {
        var cache = new WeakMap();
        def(proto, prop, function () {
            var l = cache.get(this);
            if (!l) { l = new Proxy(new TokenList(this, attr), { get: function (t, p, r) { return typeof p === 'string' && /^(0|[1-9][0-9]*)$/.test(p) ? tokens(t)[Number(p)] : Reflect.get(t, p, r); } }); cache.set(this, l); }
            return l;
        }, function (v) { this.setAttribute(attr, String(v)); });
    }

    var P = function (name) { var C = g[name]; return C && C.prototype; };

    // ---- every HTMLElement
    var H = g.HTMLElement.prototype;
    str(H, 'title', 'title');
    str(H, 'lang', 'lang');
    str(H, 'accessKey', 'accesskey');
    str(H, 'nonce', 'nonce');
    enm(H, 'dir', 'dir', ['ltr', 'rtl', 'auto'], '', '');
    bool(H, 'hidden', 'hidden');
    bool(H, 'inert', 'inert');
    bool(H, 'autofocus', 'autofocus');
    def(H, 'translate', function () { return this.getAttribute('translate') !== 'no'; }, function (v) { this.setAttribute('translate', v ? 'yes' : 'no'); });
    def(H, 'draggable', function () { return this.getAttribute('draggable') === 'true' || (this.getAttribute('draggable') === null && (this.localName === 'img' || (this.localName === 'a' && this.hasAttribute('href')))); },
        function (v) { this.setAttribute('draggable', v ? 'true' : 'false'); });
    def(H, 'spellcheck', function () { return this.getAttribute('spellcheck') !== 'false'; }, function (v) { this.setAttribute('spellcheck', v ? 'true' : 'false'); });
    def(H, 'contentEditable', function () { var v = this.getAttribute('contenteditable'); return v === null ? 'inherit' : (v === '' ? 'true' : v.toLowerCase()); },
        function (v) { this.setAttribute('contenteditable', String(v)); });
    def(H, 'isContentEditable', function () {
        for (var n = this; n && n.nodeType === 1; n = n.parentNode) {
            var v = n.getAttribute && n.getAttribute('contenteditable');
            if (v === '' || (v && v.toLowerCase() === 'true')) return true;
            if (v && v.toLowerCase() === 'false') return false;
        }
        return false;
    });
    // tabIndex: the attribute, else 0 for the elements that are focusable by default, else -1.
    var FOCUSABLE = { a: 1, area: 1, button: 1, input: 1, select: 1, textarea: 1, iframe: 1, summary: 1, details: 0, object: 1, embed: 1 };
    def(H, 'tabIndex', function () {
        var n = parseInt(this.getAttribute('tabindex'), 10);
        if (!isNaN(n)) return n;
        var t = this.localName;
        if ((t === 'a' || t === 'area') && !this.hasAttribute('href')) return -1;
        return FOCUSABLE[t] ? 0 : -1;
    }, function (v) { this.setAttribute('tabindex', String(Math.trunc(+v) || 0)); });
    def(g.Element.prototype, 'slot', function () { var v = this.getAttribute('slot'); return v === null ? '' : v; }, function (v) { this.setAttribute('slot', String(v)); });

    // ---- <link>
    var L = P('HTMLLinkElement');
    if (L) {
        url(L, 'href', 'href'); str(L, 'rel', 'rel'); list(L, 'relList', 'rel'); str(L, 'type', 'type'); str(L, 'media', 'media');
        str(L, 'as', 'as'); str(L, 'hreflang', 'hreflang'); str(L, 'integrity', 'integrity'); str(L, 'imageSrcset', 'imagesrcset');
        str(L, 'imageSizes', 'imagesizes'); cors(L); bool(L, 'disabled', 'disabled'); list(L, 'sizes', 'sizes');
        enm(L, 'referrerPolicy', 'referrerpolicy', ['', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'], '', '');
        enm(L, 'fetchPriority', 'fetchpriority', ['high', 'low', 'auto'], 'auto');
    }
    // ---- <base>
    var B = P('HTMLBaseElement');
    if (B) { url(B, 'href', 'href'); str(B, 'target', 'target'); }
    // ---- <meta>
    var M = P('HTMLMetaElement');
    if (M) { str(M, 'name', 'name'); str(M, 'content', 'content'); str(M, 'httpEquiv', 'http-equiv'); str(M, 'media', 'media'); str(M, 'charset', 'charset'); }
    // ---- <style>
    var S = P('HTMLStyleElement');
    if (S) { str(S, 'media', 'media'); str(S, 'type', 'type'); bool(S, 'disabled', 'disabled'); }
    // ---- <script>
    var SC = P('HTMLScriptElement');
    if (SC) {
        str(SC, 'type', 'type'); str(SC, 'charset', 'charset'); str(SC, 'integrity', 'integrity'); cors(SC);
        bool(SC, 'defer', 'defer'); bool(SC, 'noModule', 'nomodule');
        def(SC, 'async', function () { return this.hasAttribute('async'); }, function (v) { if (v) this.setAttribute('async', ''); else this.removeAttribute('async'); });
        enm(SC, 'referrerPolicy', 'referrerpolicy', ['', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'], '', '');
        enm(SC, 'fetchPriority', 'fetchpriority', ['high', 'low', 'auto'], 'auto');
        def(SC, 'text', function () { return this.textContent; }, function (v) { this.textContent = String(v); });
    }
    // ---- <img>
    var I = P('HTMLImageElement');
    if (I) {
        str(I, 'alt', 'alt'); str(I, 'srcset', 'srcset'); str(I, 'sizes', 'sizes'); cors(I); str(I, 'useMap', 'usemap');
        bool(I, 'isMap', 'ismap');
        enm(I, 'loading', 'loading', ['eager', 'lazy'], 'eager');
        enm(I, 'decoding', 'decoding', ['sync', 'async', 'auto'], 'auto');
        enm(I, 'referrerPolicy', 'referrerpolicy', ['', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'], '', '');
        enm(I, 'fetchPriority', 'fetchpriority', ['high', 'low', 'auto'], 'auto');
        def(I, 'currentSrc', function () { var v = this.getAttribute('src'); return v === null ? '' : resolve(v); });
        def(I, 'complete', function () { var v = this.getAttribute('src'); return v === null || v === ''; });
        def(I, 'naturalWidth', function () { return 0; });
        def(I, 'naturalHeight', function () { return 0; });
        def(I, 'x', function () { return this.getBoundingClientRect().x; });
        def(I, 'y', function () { return this.getBoundingClientRect().y; });
        method(I, 'decode', function decode() { return Promise.resolve(); });
    }
    function method(proto, name, fn) {
        if (!proto || has(proto, name)) return;
        Object.defineProperty(proto, name, { value: fn, writable: true, configurable: true, enumerable: true });
    }
    // ---- <a> and <area> (the URL parts are web_history.js's)
    ['HTMLAnchorElement', 'HTMLAreaElement'].forEach(function (name) {
        var A = P(name);
        if (!A) return;
        str(A, 'target', 'target'); str(A, 'download', 'download'); str(A, 'ping', 'ping'); str(A, 'rel', 'rel'); list(A, 'relList', 'rel');
        str(A, 'hreflang', 'hreflang'); str(A, 'type', 'type');
        enm(A, 'referrerPolicy', 'referrerpolicy', ['', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'], '', '');
    });
    var AA = P('HTMLAnchorElement');
    if (AA) def(AA, 'text', function () { return this.textContent; }, function (v) { this.textContent = String(v); });
    // ---- <iframe>
    var F = P('HTMLIFrameElement');
    if (F) {
        url(F, 'src', 'src'); str(F, 'name', 'name'); str(F, 'srcdoc', 'srcdoc'); str(F, 'allow', 'allow'); list(F, 'sandbox', 'sandbox');
        bool(F, 'allowFullscreen', 'allowfullscreen');
        enm(F, 'loading', 'loading', ['eager', 'lazy'], 'eager');
        enm(F, 'referrerPolicy', 'referrerpolicy', ['', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'], '', '');
        str(F, 'width', 'width'); str(F, 'height', 'height');
        def(F, 'contentWindow', function () { return null; });
        def(F, 'contentDocument', function () { return null; });
    }
    // ---- <form>: the submission attributes (the controls are web_forms.js's)
    var FO = P('HTMLFormElement');
    if (FO) {
        def(FO, 'action', function () { var v = this.getAttribute('action'); return v === null || v === '' ? String(document.URL || g.location.href) : resolve(v); },
            function (v) { this.setAttribute('action', String(v)); });
        enm(FO, 'method', 'method', ['get', 'post', 'dialog'], 'get');
        enm(FO, 'enctype', 'enctype', ['application/x-www-form-urlencoded', 'multipart/form-data', 'text/plain'], 'application/x-www-form-urlencoded');
        def(FO, 'encoding', function () { return this.enctype; }, function (v) { this.enctype = v; });
        str(FO, 'target', 'target'); str(FO, 'acceptCharset', 'accept-charset'); str(FO, 'autocomplete', 'autocomplete', 'on');
        bool(FO, 'noValidate', 'novalidate'); str(FO, 'name', 'name');
    }
})(typeof globalThis === 'object' ? globalThis : this);
