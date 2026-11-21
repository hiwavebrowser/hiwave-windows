// ECMAScript members Boa 0.20 does not provide, that pages call without
// feature-testing (ES Annex B and ES2024/2025 additions):
//   String.prototype.substr, trimLeft, trimRight and the HTML-wrapping methods
//   (anchor, big, blink, bold, fixed, fontcolor, fontsize, italics, link,
//   small, strike, sub, sup); the Set methods
//   (union, intersection, difference, symmetricDifference, isSubsetOf,
//   isSupersetOf, isDisjointFrom); Array.fromAsync; FinalizationRegistry.
//
// Why: github's favicon updater calls `href.substr(...)` at load; `substr` was
// not a function, so its whole module threw "not a callable function" and
// every behaviour the page registers after it never ran. Each name is defined
// only when the engine does not have it.
(function (g) {
    'use strict';
    function define(obj, name, fn) {
        if (typeof obj[name] === 'function') return;
        Object.defineProperty(obj, name, { value: fn, writable: true, configurable: true, enumerable: false });
    }
    function thisString(v, method) {
        if (v === null || v === undefined) {
            throw new TypeError('String.prototype.' + method + ' called on null or undefined');
        }
        return String(v);
    }
    function toInt(v) {
        v = Number(v);
        if (v !== v) return 0;
        return v < 0 ? Math.ceil(v) : Math.floor(v);
    }

    var S = String.prototype;
    define(S, 'substr', function substr(start, length) {
        var s = thisString(this, 'substr'), size = s.length;
        var from = toInt(start);
        if (from === -Infinity) from = 0; else if (from < 0) from = Math.max(size + from, 0); else from = Math.min(from, size);
        var count = length === undefined ? size - from : toInt(length);
        count = Math.min(Math.max(count, 0), size - from);
        return count <= 0 ? '' : s.slice(from, from + count);
    });
    define(S, 'trimLeft', function trimLeft() { return thisString(this, 'trimLeft').trimStart(); });
    define(S, 'trimRight', function trimRight() { return thisString(this, 'trimRight').trimEnd(); });
    // CreateHTML: <tag attr="value">string</tag>, with " in the value escaped.
    function html(name, tag, attr) {
        define(S, name, function (value) {
            var s = thisString(this, name), open = '<' + tag;
            if (attr) open += ' ' + attr + '="' + String(value).replace(/"/g, '&quot;') + '"';
            return open + '>' + s + '</' + tag + '>';
        });
    }
    html('anchor', 'a', 'name'); html('big', 'big'); html('blink', 'blink'); html('bold', 'b'); html('fixed', 'tt');
    html('fontcolor', 'font', 'color'); html('fontsize', 'font', 'size'); html('italics', 'i'); html('link', 'a', 'href');
    html('small', 'small'); html('strike', 'strike'); html('sub', 'sub'); html('sup', 'sup');

    // ---- Set methods (ES2025)
    if (typeof Set === 'function') {
        var SP = Set.prototype;
        function record(other, method) {
            if (other === null || typeof other !== 'object' || typeof other.has !== 'function' || typeof other.keys !== 'function') {
                throw new TypeError("Failed to execute '" + method + "' on 'Set': The argument must be set-like.");
            }
            return other;
        }
        function toArray(other) { var out = [], it = other.keys(), r; while (!(r = it.next()).done) out.push(r.value); return out; }
        define(SP, 'union', function union(other) { record(other, 'union'); var out = new Set(this); toArray(other).forEach(function (v) { out.add(v); }); return out; });
        define(SP, 'intersection', function intersection(other) { record(other, 'intersection'); var out = new Set(); this.forEach(function (v) { if (other.has(v)) out.add(v); }); return out; });
        define(SP, 'difference', function difference(other) { record(other, 'difference'); var out = new Set(); this.forEach(function (v) { if (!other.has(v)) out.add(v); }); return out; });
        define(SP, 'symmetricDifference', function symmetricDifference(other) {
            record(other, 'symmetricDifference');
            var out = new Set(this), mine = this;
            toArray(other).forEach(function (v) { if (mine.has(v)) out.delete(v); else out.add(v); });
            return out;
        });
        define(SP, 'isSubsetOf', function isSubsetOf(other) { record(other, 'isSubsetOf'); var ok = true; this.forEach(function (v) { if (!other.has(v)) ok = false; }); return ok; });
        define(SP, 'isSupersetOf', function isSupersetOf(other) { record(other, 'isSupersetOf'); var mine = this; return toArray(other).every(function (v) { return mine.has(v); }); });
        define(SP, 'isDisjointFrom', function isDisjointFrom(other) { record(other, 'isDisjointFrom'); var ok = true; this.forEach(function (v) { if (other.has(v)) ok = false; }); return ok; });
    }

    // ---- Array.fromAsync (ES2024): an iterable or async iterable, or an array-like of promises.
    if (typeof Array.fromAsync !== 'function') {
        Object.defineProperty(Array, 'fromAsync', {
            value: function fromAsync(items, mapFn, thisArg) {
                var C = typeof this === 'function' ? this : Array;
                return (async function () {
                    var out = [], i = 0;
                    if (items != null && typeof items[Symbol.asyncIterator] === 'function') {
                        for await (var v of items) { out.push(mapFn ? await mapFn.call(thisArg, v, i) : v); i++; }
                    } else if (items != null && typeof items[Symbol.iterator] === 'function') {
                        for (var w of items) { var x = await w; out.push(mapFn ? await mapFn.call(thisArg, x, i) : x); i++; }
                    } else {
                        var n = items == null ? 0 : (items.length >>> 0);
                        for (; i < n; i++) { var y = await items[i]; out.push(mapFn ? await mapFn.call(thisArg, y, i) : y); }
                    }
                    return C === Array ? out : Object.assign(new C(), out);
                })();
            },
            writable: true, configurable: true, enumerable: false
        });
    }

    // ---- FinalizationRegistry: nothing here observes a collection, so callbacks never run
    // (which the specification allows) and register/unregister only keep the books.
    if (typeof g.FinalizationRegistry !== 'function') {
        var FinalizationRegistry = function FinalizationRegistry(callback) {
            if (!(this instanceof FinalizationRegistry)) throw new TypeError("Constructor FinalizationRegistry requires 'new'");
            if (typeof callback !== 'function') throw new TypeError('FinalizationRegistry: cleanup must be callable');
            Object.defineProperty(this, '_tokens', { value: new Map() });
        };
        FinalizationRegistry.prototype.register = function register(target, held, token) {
            if (target === null || (typeof target !== 'object' && typeof target !== 'function')) throw new TypeError('FinalizationRegistry.prototype.register: invalid target');
            if (token !== undefined) this._tokens.set(token, true);
        };
        FinalizationRegistry.prototype.unregister = function unregister(token) { return this._tokens.delete(token); };
        Object.defineProperty(FinalizationRegistry.prototype, Symbol.toStringTag, { value: 'FinalizationRegistry', configurable: true });
        Object.defineProperty(g, 'FinalizationRegistry', { value: FinalizationRegistry, writable: true, configurable: true, enumerable: false });
    }
})(typeof globalThis === 'object' ? globalThis : this);
