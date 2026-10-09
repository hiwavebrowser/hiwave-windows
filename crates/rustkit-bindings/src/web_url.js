// URL and URLSearchParams (WHATWG URL Standard).
//
// Parsing and the component setters are the Rust `url` crate's, reached
// through two host functions, so the corner cases (default ports, IDNA,
// percent-encoding sets, relative resolution, special-scheme path rules)
// are the same ones the network stack uses. This file is the thin object
// layer: the accessors, `searchParams` kept in step with `search`, and
// `URLSearchParams` itself, which is pure string work.
(function (g) {
    var parse = g.__rustkit_url_parse;
    var update = g.__rustkit_url_update;
    delete g.__rustkit_url_parse;
    delete g.__rustkit_url_update;
    if (typeof g.URL === 'function' && typeof g.URLSearchParams === 'function') return;

    var HEX = '0123456789ABCDEF';
    // application/x-www-form-urlencoded byte serializer: space is '+', the
    // unreserved set is A-Z a-z 0-9 * - . _ and everything else is %XX.
    function formEncode(s) {
        return encodeURIComponent(String(s)).replace(/[!'()~]/g, function (c) {
            var n = c.charCodeAt(0);
            return '%' + HEX[n >> 4] + HEX[n & 15];
        }).replace(/%20/g, '+');
    }
    function formDecode(s) {
        s = s.replace(/\+/g, ' ');
        try { return decodeURIComponent(s); }
        catch (e) {
            // Malformed escapes pass through, valid ones still decode.
            return s.replace(/%[0-9a-fA-F]{2}/g, function (m) {
                try { return decodeURIComponent(m); } catch (e2) { return m; }
            });
        }
    }
    function parseQuery(q) {
        var out = [];
        if (q.charAt(0) === '?') q = q.slice(1);
        if (q === '') return out;
        q.split('&').forEach(function (pair) {
            if (pair === '') return;
            var i = pair.indexOf('=');
            var k = i < 0 ? pair : pair.slice(0, i);
            var v = i < 0 ? '' : pair.slice(i + 1);
            out.push([formDecode(k), formDecode(v)]);
        });
        return out;
    }

    var LIST = typeof Symbol === 'function' ? Symbol('list') : '__list';
    var OWNER = typeof Symbol === 'function' ? Symbol('url') : '__url';

    function URLSearchParams(init) {
        if (!(this instanceof URLSearchParams)) throw new TypeError("Failed to construct 'URLSearchParams': Please use the 'new' operator");
        var list = [];
        if (init === undefined || init === null) {
            // empty
        } else if (typeof init === 'string') {
            list = parseQuery(init);
        } else if (typeof init === 'object' || typeof init === 'function') {
            if (init instanceof URLSearchParams) {
                list = init[LIST].map(function (p) { return [p[0], p[1]]; });
            } else if (typeof init[Symbol.iterator] === 'function') {
                Array.from(init).forEach(function (pair) {
                    pair = Array.from(pair);
                    if (pair.length !== 2) throw new TypeError("Failed to construct 'URLSearchParams': Each query pair must be an iterable [name, value] tuple");
                    list.push([String(pair[0]), String(pair[1])]);
                });
            } else {
                Object.keys(init).forEach(function (k) { list.push([k, String(init[k])]); });
            }
        } else {
            list = parseQuery(String(init));
        }
        Object.defineProperty(this, LIST, { value: list, writable: true });
        Object.defineProperty(this, OWNER, { value: null, writable: true });
    }
    function changed(p) {
        var o = p[OWNER];
        if (o) o._setSearchFromParams(p.toString());
    }
    var UP = URLSearchParams.prototype;
    UP.append = function (name, value) { this[LIST].push([String(name), String(value)]); changed(this); };
    UP['delete'] = function (name, value) {
        name = String(name);
        this[LIST] = this[LIST].filter(function (p) {
            return !(p[0] === name && (value === undefined || p[1] === String(value)));
        });
        changed(this);
    };
    UP.get = function (name) {
        name = String(name);
        for (var i = 0; i < this[LIST].length; i++) if (this[LIST][i][0] === name) return this[LIST][i][1];
        return null;
    };
    UP.getAll = function (name) {
        name = String(name);
        return this[LIST].filter(function (p) { return p[0] === name; }).map(function (p) { return p[1]; });
    };
    UP.has = function (name, value) {
        name = String(name);
        return this[LIST].some(function (p) { return p[0] === name && (value === undefined || p[1] === String(value)); });
    };
    UP.set = function (name, value) {
        name = String(name); value = String(value);
        var list = this[LIST], found = false, out = [];
        list.forEach(function (p) {
            if (p[0] === name) { if (!found) { found = true; out.push([name, value]); } }
            else out.push(p);
        });
        if (!found) out.push([name, value]);
        this[LIST] = out;
        changed(this);
    };
    UP.sort = function () {
        // Stable sort by UTF-16 code unit order of the name.
        var idx = this[LIST].map(function (p, i) { return [p, i]; });
        idx.sort(function (a, b) { return a[0][0] < b[0][0] ? -1 : a[0][0] > b[0][0] ? 1 : a[1] - b[1]; });
        this[LIST] = idx.map(function (x) { return x[0]; });
        changed(this);
    };
    UP.forEach = function (cb, thisArg) {
        if (typeof cb !== 'function') throw new TypeError("Failed to execute 'forEach' on 'URLSearchParams': parameter 1 is not of type 'Function'.");
        for (var i = 0; i < this[LIST].length; i++) cb.call(thisArg, this[LIST][i][1], this[LIST][i][0], this);
    };
    UP.toString = function () {
        return this[LIST].map(function (p) { return formEncode(p[0]) + '=' + formEncode(p[1]); }).join('&');
    };
    function iterator(self, kind) {
        var i = 0;
        var it = {
            next: function () {
                if (i >= self[LIST].length) return { value: undefined, done: true };
                var p = self[LIST][i++];
                return { value: kind === 'keys' ? p[0] : kind === 'values' ? p[1] : [p[0], p[1]], done: false };
            }
        };
        if (typeof Symbol === 'function') {
            it[Symbol.iterator] = function () { return this; };
            it[Symbol.toStringTag] = 'URLSearchParams Iterator';
        }
        return it;
    }
    UP.keys = function () { return iterator(this, 'keys'); };
    UP.values = function () { return iterator(this, 'values'); };
    UP.entries = function () { return iterator(this, 'entries'); };
    if (typeof Symbol === 'function') {
        UP[Symbol.iterator] = UP.entries;
        Object.defineProperty(UP, Symbol.toStringTag, { value: 'URLSearchParams', configurable: true });
    }
    Object.defineProperty(UP, 'size', { get: function () { return this[LIST].length; }, configurable: true });

    var FIELDS = ['href', 'origin', 'protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash'];

    function URL(url, base) {
        if (!(this instanceof URL)) throw new TypeError("Failed to construct 'URL': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        if (arguments.length === 0) throw new TypeError("Failed to construct 'URL': 1 argument required, but only 0 present.");
        var r = parse(String(url), base === undefined ? null : String(base));
        if (r === null) throw new TypeError("Failed to construct 'URL': Invalid URL");
        Object.defineProperty(this, '_c', { value: JSON.parse(r), writable: true });
        var sp = new URLSearchParams(this._c.search);
        sp[OWNER] = this;
        Object.defineProperty(this, '_sp', { value: sp });
    }
    var UPROTO = URL.prototype;
    function commit(self, field, value) {
        var r = update(self._c.href, field, String(value));
        if (r === null) {
            // href is the one setter that throws; the rest ignore a bad value.
            if (field === 'href') throw new TypeError("Failed to set the 'href' property on 'URL': Invalid URL");
            return;
        }
        self._c = JSON.parse(r);
        if (field !== 'search') self._sp[LIST] = parseQuery(self._c.search);
        else self._sp[LIST] = parseQuery(self._c.search);
    }
    UPROTO._setSearchFromParams = function (qs) {
        var r = update(this._c.href, 'search', qs === '' ? '' : '?' + qs);
        if (r !== null) this._c = JSON.parse(r);
    };
    FIELDS.forEach(function (f) {
        Object.defineProperty(UPROTO, f, {
            get: function () { return this._c[f]; },
            set: f === 'origin' ? undefined : function (v) { commit(this, f, v); },
            enumerable: true, configurable: true
        });
    });
    Object.defineProperty(UPROTO, 'searchParams', { get: function () { return this._sp; }, enumerable: true, configurable: true });
    if (typeof Symbol === 'function') Object.defineProperty(UPROTO, Symbol.toStringTag, { value: 'URL', configurable: true });
    UPROTO.toString = function () { return this._c.href; };
    UPROTO.toJSON = function () { return this._c.href; };
    function needArg(n, method) {
        if (n === 0) throw new TypeError("Failed to execute '" + method + "' on 'URL': 1 argument required, but only 0 present.");
    }
    URL.canParse = function (url, base) {
        needArg(arguments.length, 'canParse');
        return parse(String(url), base === undefined ? null : String(base)) !== null;
    };
    URL.parse = function (url, base) {
        needArg(arguments.length, 'parse');
        try { return new URL(url, base); } catch (e) { return null; }
    };
    var blobCount = 0;
    URL.createObjectURL = function () { return 'blob:' + (g.location && g.location.origin || 'null') + '/' + (++blobCount); };
    URL.revokeObjectURL = function () {};

    g.URL = URL;
    g.URLSearchParams = URLSearchParams;
    if (g.webkitURL === undefined) g.webkitURL = URL;
})(globalThis);
