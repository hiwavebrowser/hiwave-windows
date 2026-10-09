// Blob, File, FormData, AbortController, AbortSignal, structuredClone.
// Pure data and event plumbing; nothing here touches the network or the
// file system. Each name is only defined when nothing else defines it.
(function (g) {
    function def(name, value) {
        if (g[name] === undefined) {
            Object.defineProperty(g, name, { value: value, writable: true, configurable: true, enumerable: false });
        }
    }
    function domError(message, name) {
        var D = g.DOMException;
        if (typeof D === 'function') return new D(message, name);
        var e = new Error(message); e.name = name; return e;
    }
    function utf8(str) {
        return typeof g.TextEncoder === 'function' ? new g.TextEncoder().encode(str) : (function () {
            var out = [];
            for (var i = 0; i < str.length; i++) {
                var c = str.charCodeAt(i);
                if (c < 0x80) out.push(c);
                else if (c < 0x800) out.push(0xC0 | (c >> 6), 0x80 | (c & 63));
                else out.push(0xE0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
            }
            return new Uint8Array(out);
        })();
    }
    function fromUtf8(bytes) {
        return typeof g.TextDecoder === 'function' ? new g.TextDecoder().decode(bytes) : String.fromCharCode.apply(null, bytes);
    }

    // ---- Blob (File API §3). Stored as one Uint8Array.
    var BYTES = typeof Symbol === 'function' ? Symbol('bytes') : '__bytes';
    function partBytes(part) {
        if (part instanceof Blob) return part[BYTES];
        if (part instanceof ArrayBuffer) return new Uint8Array(part.slice(0));
        if (ArrayBuffer.isView(part)) return new Uint8Array(part.buffer.slice(part.byteOffset, part.byteOffset + part.byteLength));
        return utf8(String(part));
    }
    function Blob(parts, options) {
        if (!(this instanceof Blob)) throw new TypeError("Failed to construct 'Blob': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        var chunks = [], total = 0;
        if (parts !== undefined && parts !== null) {
            if (typeof parts !== 'object' && typeof parts !== 'function' || typeof parts[Symbol.iterator] !== 'function') {
                throw new TypeError("Failed to construct 'Blob': The provided value cannot be converted to a sequence.");
            }
            Array.from(parts).forEach(function (p) { var b = partBytes(p); chunks.push(b); total += b.length; });
        }
        var bytes = new Uint8Array(total), at = 0;
        chunks.forEach(function (b) { bytes.set(b, at); at += b.length; });
        var type = options && options.type !== undefined ? String(options.type) : '';
        Object.defineProperty(this, BYTES, { value: bytes, writable: true });
        Object.defineProperty(this, '_type', { value: /[^ -~]/.test(type) ? '' : type.toLowerCase() });
    }
    Object.defineProperty(Blob.prototype, 'size', { get: function () { return this[BYTES].length; }, enumerable: true, configurable: true });
    Object.defineProperty(Blob.prototype, 'type', { get: function () { return this._type; }, enumerable: true, configurable: true });
    function relIndex(v, len, dflt) {
        if (v === undefined) return dflt;
        v = Math.trunc(Number(v)) || 0;
        return v < 0 ? Math.max(len + v, 0) : Math.min(v, len);
    }
    Blob.prototype.slice = function (start, end, contentType) {
        var len = this[BYTES].length, s = relIndex(start, len, 0), e = relIndex(end, len, len);
        var out = new Blob([], { type: contentType === undefined ? '' : contentType });
        out[BYTES] = this[BYTES].slice(s, Math.max(s, e));
        return out;
    };
    Blob.prototype.text = function () { var self = this; return Promise.resolve().then(function () { return fromUtf8(self[BYTES]); }); };
    Blob.prototype.arrayBuffer = function () {
        var self = this;
        return Promise.resolve().then(function () { return self[BYTES].buffer.slice(self[BYTES].byteOffset, self[BYTES].byteOffset + self[BYTES].byteLength); });
    };
    Blob.prototype.bytes = function () { var self = this; return Promise.resolve().then(function () { return new Uint8Array(self[BYTES]); }); };
    if (typeof Symbol === 'function') Object.defineProperty(Blob.prototype, Symbol.toStringTag, { value: 'Blob', configurable: true });
    def('Blob', Blob);

    // ---- File (File API §4)
    function File(parts, name, options) {
        if (!(this instanceof File)) throw new TypeError("Failed to construct 'File': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        if (arguments.length < 2) throw new TypeError("Failed to construct 'File': 2 arguments required, but only " + arguments.length + " present.");
        Blob.call(this, parts, options);
        Object.defineProperty(this, '_name', { value: String(name).replace(/\//g, ':') });
        Object.defineProperty(this, '_lm', { value: options && options.lastModified !== undefined ? Number(options.lastModified) : Date.now() });
    }
    File.prototype = Object.create(Blob.prototype, { constructor: { value: File, writable: true, configurable: true } });
    Object.defineProperty(File.prototype, 'name', { get: function () { return this._name; }, enumerable: true, configurable: true });
    Object.defineProperty(File.prototype, 'lastModified', { get: function () { return this._lm; }, enumerable: true, configurable: true });
    Object.defineProperty(File.prototype, 'webkitRelativePath', { get: function () { return ''; }, enumerable: true, configurable: true });
    if (typeof Symbol === 'function') Object.defineProperty(File.prototype, Symbol.toStringTag, { value: 'File', configurable: true });
    def('File', File);

    // ---- FormData (XHR §5.2). An ordered list of [name, string | File].
    var ENTRIES = typeof Symbol === 'function' ? Symbol('entries') : '__entries';
    function entryValue(value, filename) {
        if (value instanceof Blob) {
            if (value instanceof File && filename === undefined) return value;
            var f = new File([value], filename !== undefined ? String(filename) : (value instanceof File ? value.name : 'blob'), { type: value.type });
            return f;
        }
        return String(value);
    }
    function FormData(form) {
        if (!(this instanceof FormData)) throw new TypeError("Failed to construct 'FormData': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        Object.defineProperty(this, ENTRIES, { value: [], writable: true });
        if (form !== undefined && form !== null) {
            // A <form>: its named, enabled controls (text-like values only).
            var els = form.elements || (form.querySelectorAll ? form.querySelectorAll('input, select, textarea') : []);
            for (var i = 0; i < els.length; i++) {
                var el = els[i], n = el.getAttribute && el.getAttribute('name');
                if (!n || (el.hasAttribute && el.hasAttribute('disabled'))) continue;
                var t = String(el.type || '').toLowerCase();
                if (t === 'checkbox' || t === 'radio') { if (!el.checked) continue; }
                else if (t === 'file' || t === 'submit' || t === 'button' || t === 'reset' || t === 'image') continue;
                this[ENTRIES].push([n, String(el.value === undefined ? '' : el.value)]);
            }
        }
    }
    var FP = FormData.prototype;
    FP.append = function (name, value, filename) { this[ENTRIES].push([String(name), entryValue(value, filename)]); };
    FP['delete'] = function (name) { name = String(name); this[ENTRIES] = this[ENTRIES].filter(function (e) { return e[0] !== name; }); };
    FP.get = function (name) { name = String(name); for (var i = 0; i < this[ENTRIES].length; i++) if (this[ENTRIES][i][0] === name) return this[ENTRIES][i][1]; return null; };
    FP.getAll = function (name) { name = String(name); return this[ENTRIES].filter(function (e) { return e[0] === name; }).map(function (e) { return e[1]; }); };
    FP.has = function (name) { name = String(name); return this[ENTRIES].some(function (e) { return e[0] === name; }); };
    FP.set = function (name, value, filename) {
        name = String(name);
        var v = entryValue(value, filename), found = false, out = [];
        this[ENTRIES].forEach(function (e) {
            if (e[0] === name) { if (!found) { found = true; out.push([name, v]); } } else out.push(e);
        });
        if (!found) out.push([name, v]);
        this[ENTRIES] = out;
    };
    FP.forEach = function (cb, thisArg) {
        if (typeof cb !== 'function') throw new TypeError("Failed to execute 'forEach' on 'FormData': parameter 1 is not of type 'Function'.");
        for (var i = 0; i < this[ENTRIES].length; i++) cb.call(thisArg, this[ENTRIES][i][1], this[ENTRIES][i][0], this);
    };
    function fdIter(self, kind) {
        var i = 0;
        var it = { next: function () {
            if (i >= self[ENTRIES].length) return { value: undefined, done: true };
            var e = self[ENTRIES][i++];
            return { value: kind === 'keys' ? e[0] : kind === 'values' ? e[1] : [e[0], e[1]], done: false };
        } };
        if (typeof Symbol === 'function') it[Symbol.iterator] = function () { return this; };
        return it;
    }
    FP.keys = function () { return fdIter(this, 'keys'); };
    FP.values = function () { return fdIter(this, 'values'); };
    FP.entries = function () { return fdIter(this, 'entries'); };
    if (typeof Symbol === 'function') FP[Symbol.iterator] = FP.entries;
    def('FormData', FormData);

    // ---- AbortSignal / AbortController (DOM §3.1-3.2)
    var LISTENERS = typeof Symbol === 'function' ? Symbol('listeners') : '__listeners';
    function AbortSignal() {
        throw new TypeError('Illegal constructor');
    }
    function makeSignal() {
        var s = Object.create(AbortSignal.prototype);
        Object.defineProperty(s, LISTENERS, { value: [], writable: true });
        Object.defineProperty(s, '_aborted', { value: false, writable: true });
        Object.defineProperty(s, '_reason', { value: undefined, writable: true });
        s.onabort = null;
        return s;
    }
    Object.defineProperty(AbortSignal.prototype, 'aborted', { get: function () { return this._aborted; }, enumerable: true, configurable: true });
    Object.defineProperty(AbortSignal.prototype, 'reason', { get: function () { return this._reason; }, enumerable: true, configurable: true });
    AbortSignal.prototype.throwIfAborted = function () { if (this._aborted) throw this._reason; };
    AbortSignal.prototype.addEventListener = function (type, cb, options) {
        if (type !== 'abort' || typeof cb !== 'function' && !(cb && typeof cb.handleEvent === 'function')) return;
        var once = !!(options && typeof options === 'object' && options.once);
        for (var i = 0; i < this[LISTENERS].length; i++) if (this[LISTENERS][i].cb === cb) return;
        this[LISTENERS].push({ cb: cb, once: once });
    };
    AbortSignal.prototype.removeEventListener = function (type, cb) {
        if (type !== 'abort') return;
        this[LISTENERS] = this[LISTENERS].filter(function (l) { return l.cb !== cb; });
    };
    AbortSignal.prototype.dispatchEvent = function (evt) {
        var self = this;
        if (typeof this.onabort === 'function') { try { this.onabort.call(this, evt); } catch (e) { report(e); } }
        this[LISTENERS].slice().forEach(function (l) {
            if (l.once) self.removeEventListener('abort', l.cb);
            try { typeof l.cb === 'function' ? l.cb.call(self, evt) : l.cb.handleEvent(evt); } catch (e) { report(e); }
        });
        return true;
    };
    function report(e) { if (typeof g.console === 'object' && g.console.error) g.console.error(e); }
    function abortSignal(signal, reason) {
        if (signal._aborted) return;
        signal._aborted = true;
        signal._reason = reason === undefined ? domError('signal is aborted without reason', 'AbortError') : reason;
        var evt = typeof g.Event === 'function' ? new g.Event('abort') : { type: 'abort' };
        signal.dispatchEvent(evt);
    }
    AbortSignal.abort = function (reason) { var s = makeSignal(); abortSignal(s, reason); return s; };
    AbortSignal.timeout = function (ms) {
        var s = makeSignal();
        g.setTimeout(function () { abortSignal(s, domError('signal timed out', 'TimeoutError')); }, Number(ms));
        return s;
    };
    AbortSignal.any = function (signals) {
        var s = makeSignal();
        var list = Array.from(signals);
        for (var i = 0; i < list.length; i++) if (list[i].aborted) { abortSignal(s, list[i].reason); return s; }
        list.forEach(function (sig) { sig.addEventListener('abort', function () { abortSignal(s, sig.reason); }); });
        return s;
    };
    if (typeof Symbol === 'function') Object.defineProperty(AbortSignal.prototype, Symbol.toStringTag, { value: 'AbortSignal', configurable: true });
    def('AbortSignal', AbortSignal);

    function AbortController() {
        if (!(this instanceof AbortController)) throw new TypeError("Failed to construct 'AbortController': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        Object.defineProperty(this, '_signal', { value: makeSignal() });
    }
    Object.defineProperty(AbortController.prototype, 'signal', { get: function () { return this._signal; }, enumerable: true, configurable: true });
    AbortController.prototype.abort = function (reason) { abortSignal(this._signal, reason); };
    if (typeof Symbol === 'function') Object.defineProperty(AbortController.prototype, Symbol.toStringTag, { value: 'AbortController', configurable: true });
    def('AbortController', AbortController);

    // ---- structuredClone (HTML §2.7). The cloneable subset; cycles preserved.
    def('structuredClone', function structuredClone(value) {
        if (arguments.length === 0) throw new TypeError("Failed to execute 'structuredClone' on 'Window': 1 argument required, but only 0 present.");
        var seen = new Map();
        function fail(what) { throw domError(what + ' could not be cloned.', 'DataCloneError'); }
        function clone(v) {
            if (v === null || (typeof v !== 'object' && typeof v !== 'function')) {
                if (typeof v === 'symbol') fail('Symbol()');
                return v;
            }
            if (typeof v === 'function') fail(String(v).slice(0, 40));
            if (seen.has(v)) return seen.get(v);
            var out;
            if (v instanceof Date) { out = new Date(v.getTime()); seen.set(v, out); return out; }
            if (v instanceof RegExp) { out = new RegExp(v.source, v.flags); seen.set(v, out); return out; }
            if (v instanceof Boolean || v instanceof Number || v instanceof String ||
                (typeof BigInt === 'function' && v instanceof BigInt)) { out = Object(v.valueOf()); seen.set(v, out); return out; }
            if (v instanceof ArrayBuffer) { out = v.slice(0); seen.set(v, out); return out; }
            if (ArrayBuffer.isView(v)) {
                var buf = clone(v.buffer);
                out = v instanceof DataView ? new DataView(buf, v.byteOffset, v.byteLength) : new v.constructor(buf, v.byteOffset, v.length);
                seen.set(v, out); return out;
            }
            if (v instanceof Blob) { seen.set(v, v); return v; }
            if (v instanceof Map) { out = new Map(); seen.set(v, out); v.forEach(function (val, key) { out.set(clone(key), clone(val)); }); return out; }
            if (v instanceof Set) { out = new Set(); seen.set(v, out); v.forEach(function (val) { out.add(clone(val)); }); return out; }
            if (v instanceof Error) {
                var C = { EvalError: EvalError, RangeError: RangeError, ReferenceError: ReferenceError, SyntaxError: SyntaxError, TypeError: TypeError, URIError: URIError }[v.name] || Error;
                out = new C(v.message); seen.set(v, out);
                if (v.stack !== undefined) { try { out.stack = v.stack; } catch (e) {} }
                return out;
            }
            if (Array.isArray(v)) {
                out = new Array(v.length); seen.set(v, out);
                Object.keys(v).forEach(function (k) { out[k] = clone(v[k]); });
                return out;
            }
            if (v.nodeType !== undefined && typeof v.cloneNode === 'function') fail('A DOM node');
            if (typeof Promise === 'function' && v instanceof Promise) fail('#<Promise>');
            if (typeof WeakMap === 'function' && (v instanceof WeakMap || v instanceof WeakSet)) fail('#<' + (v instanceof WeakMap ? 'WeakMap' : 'WeakSet') + '>');
            out = {}; seen.set(v, out);
            Object.keys(v).forEach(function (k) { out[k] = clone(v[k]); });
            return out;
        }
        return clone(value);
    });
})(globalThis);
