// The structured clone algorithm for postMessage (STR-4 W4): `__rk_sc.serialize(value, transfer,
// where)` turns a value graph into a string, `__rk_sc.deserialize(text)` makes the copy. The
// same file runs on the page and in a worker's own runtime; the string is what crosses threads.
//
// Cloned: primitives (undefined, -0, NaN, Infinity, bigint), plain objects and arrays (own
// enumerable string keys, getters invoked, holes kept, extra array properties kept, class
// instances become plain objects), Date, RegExp (lastIndex reset), Map, Set, Boolean/Number/String
// wrappers, Error types, ArrayBuffer, typed arrays and DataView (sharing their buffer), cycles and
// shared references. Not cloned (DataCloneError): functions, symbols, Promise, WeakMap, WeakSet,
// WeakRef, DOM nodes. A Proxy cannot be told from its target from script and is cloned as one.
//
// A transferred ArrayBuffer is serialized, then detached (`transfer()`); the receiver gets a new
// buffer with the same bytes. Only once the whole graph has serialized.
(function (g) {
    'use strict';
    // The built-ins the codec leans on, held from install: page script that later replaces them does not
    // reach a message (R1 nit on #1105). The array methods it calls are not held yet (a follow-up).
    var uncurry = Function.prototype.bind.bind(Function.prototype.call);
    var jsonParse = JSON.parse, jsonStringify = JSON.stringify, objKeys = Object.keys, isArray = Array.isArray;
    var defineProp = Object.defineProperty, objCreate = Object.create, toStr = uncurry(Object.prototype.toString);
    var indexOf = uncurry(Array.prototype.indexOf);
    var HEX = [];
    for (var h = 0; h < 256; h++) HEX.push((h < 16 ? '0' : '') + h.toString(16));
    var ERROR_NAMES = ['Error', 'EvalError', 'RangeError', 'ReferenceError', 'SyntaxError', 'TypeError', 'URIError'];
    var UNCLONEABLE = ['Promise', 'WeakMap', 'WeakSet', 'WeakRef', 'FinalizationRegistry', 'Generator', 'AsyncGenerator'];
    var VIEWS = ['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array', 'Uint32Array',
        'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array', 'Float16Array'];

    function cloneError(what, where) {
        var D = g.DOMException;
        var message = "Failed to execute 'postMessage' on '" + where + "': " + what;
        return typeof D === 'function' ? new D(message, 'DataCloneError') : new Error(message);
    }
    function describe(v) {
        try { return typeof v === 'function' ? String(v).slice(0, 40) : String(v); } catch (_) { return '#<Object>'; }
    }
    function tagOf(v) { return toStr(v).slice(8, -1); }
    function isDomNode(v) {
        return typeof g.Node === 'function' && v instanceof g.Node;
    }

    function hex(buffer) {
        var bytes = new Uint8Array(buffer), out = new Array(bytes.length);
        for (var i = 0; i < bytes.length; i++) out[i] = HEX[bytes[i]];
        return out.join('');
    }
    function unhex(text) {
        var n = text.length >> 1, bytes = new Uint8Array(n);
        for (var i = 0; i < n; i++) bytes[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
        return bytes.buffer;
    }

    function serialize(value, transfer, where) {
        where = where || 'Worker';
        var ids = new Map(), nodes = [];
        var list = [];
        if (transfer !== undefined && transfer !== null) {
            if (typeof transfer === 'object' && !isArray(transfer) && 'transfer' in transfer) transfer = transfer.transfer;
            if (transfer === undefined || transfer === null) transfer = [];
            for (var t = 0; t < transfer.length; t++) {
                var b = transfer[t];
                if (!(b instanceof g.ArrayBuffer)) throw cloneError('Value not transferable.', where);
                if (list.indexOf(b) >= 0) throw cloneError('ArrayBuffer at index ' + t + ' is a duplicate of an earlier ArrayBuffer.', where);
                if (b.detached) throw cloneError('ArrayBuffer at index ' + t + ' is already detached.', where);
                list.push(b);
            }
        }

        function node(v) {
            var known = ids.get(v);
            if (known !== undefined) return { '@': known };
            var tag = tagOf(v);
            var id = nodes.length;
            ids.set(v, id);
            var rec = {};
            nodes.push(rec);
            if (v instanceof g.Date) {
                rec.t = 'D'; rec.v = enc(v.getTime());
            } else if (v instanceof g.RegExp) {
                rec.t = 'R'; rec.s = v.source; rec.f = v.flags;
            } else if (v instanceof g.Map) {
                rec.t = 'M'; rec.e = [];
                v.forEach(function (val, key) { rec.e.push([enc(key), enc(val)]); });
            } else if (v instanceof g.Set) {
                rec.t = 'S'; rec.e = [];
                v.forEach(function (val) { rec.e.push(enc(val)); });
            } else if (v instanceof g.ArrayBuffer) {
                if (v.detached) throw cloneError('An ArrayBuffer is detached and could not be cloned.', where);
                rec.t = 'AB'; rec.d = hex(v);
            } else if (g.ArrayBuffer.isView(v)) {
                if (v.buffer.detached) throw cloneError('An ArrayBuffer is detached and could not be cloned.', where);
                rec.t = tag === 'DataView' ? 'DV' : 'TA';
                rec.c = tag; rec.b = enc(v.buffer); rec.o = v.byteOffset;
                rec.l = tag === 'DataView' ? v.byteLength : v.length;
            } else if (v instanceof g.Error) {
                var name = v.name;
                rec.t = 'E'; rec.n = indexOf(ERROR_NAMES, name) >= 0 ? name : 'Error';
                rec.m = v.message === undefined ? undefined : String(v.message);
                if (typeof v.stack === 'string') rec.s = v.stack;
            } else if (v instanceof g.Boolean || v instanceof g.Number || v instanceof g.String) {
                rec.t = 'W'; rec.k = tag; rec.v = enc(v.valueOf());
            } else if (indexOf(UNCLONEABLE, tag) >= 0 || isDomNode(v)) {
                throw cloneError(describe(v) + ' could not be cloned.', where);
            } else if (isArray(v)) {
                rec.t = 'a'; rec.l = v.length; rec.k = [];
                var keys = objKeys(v);
                for (var i = 0; i < keys.length; i++) rec.k.push([keys[i], enc(v[keys[i]])]);
            } else {
                rec.t = 'o'; rec.k = [];
                var own = objKeys(v);
                for (var j = 0; j < own.length; j++) rec.k.push([own[j], enc(v[own[j]])]);
            }
            return { '@': id };
        }
        function enc(v) {
            var type = typeof v;
            if (v === undefined) return { u: 1 };
            if (v === null || type === 'boolean' || type === 'string') return v;
            if (type === 'number') {
                if (v !== v) return { n: 'NaN' };
                if (v === Infinity) return { n: 'Inf' };
                if (v === -Infinity) return { n: '-Inf' };
                if (v === 0 && 1 / v < 0) return { z: 1 };
                return v;
            }
            if (type === 'bigint') return { b: String(v) };
            if (type === 'symbol' || type === 'function') throw cloneError(describe(v) + ' could not be cloned.', where);
            return node(v);
        }

        var root = enc(value);
        var text = jsonStringify({ r: root, n: nodes });
        // Detach only once everything serialized.
        for (var d = 0; d < list.length; d++) list[d].transfer();
        return text;
    }

    // The decoder builds only what the encoder describes: own markers, ids that are records of this
    // payload, the allowed view and error constructors, and own data properties for keys. Anything else
    // is a DataCloneError (the page turns it into a `messageerror`); nothing is built from it.
    var own = uncurry(Object.prototype.hasOwnProperty);
    function refused() {
        var D = g.DOMException;
        return typeof D === 'function'
            ? new D('The message could not be deserialized.', 'DataCloneError')
            : new Error('DataCloneError: the message could not be deserialized.');
    }
    function define(out, key, v) {
        defineProp(out, String(key), { value: v, writable: true, enumerable: true, configurable: true });
    }
    function deserialize(text) {
        var data = jsonParse(text);
        if (data === null || typeof data !== 'object' || !own(data, 'r') || !isArray(data.n)) throw refused();
        var nodes = data.n, made = objCreate(null);
        function list(v) { if (!isArray(v)) throw refused(); return v; }
        function dec(e) {
            if (e === null || typeof e !== 'object') return e;
            if (own(e, '@')) return build(e['@']);
            if (own(e, 'u')) return undefined;
            if (own(e, 'z')) return -0;
            if (own(e, 'n')) return e.n === 'NaN' ? NaN : e.n === 'Inf' ? Infinity : -Infinity;
            if (own(e, 'b')) return g.BigInt(e.b);
            throw refused();
        }
        function build(id) {
            if (typeof id !== 'number' || id !== Math.floor(id) || id < 0 || id >= nodes.length) throw refused();
            if (own(made, id)) return made[id];
            var rec = nodes[id], out, i;
            if (rec === null || typeof rec !== 'object') throw refused();
            switch (rec.t) {
                case 'o':
                    out = {}; made[id] = out;
                    list(rec.k);
                    for (i = 0; i < rec.k.length; i++) define(out, list(rec.k[i])[0], dec(rec.k[i][1]));
                    return out;
                case 'a':
                    out = new g.Array(rec.l); made[id] = out;
                    list(rec.k);
                    for (i = 0; i < rec.k.length; i++) define(out, list(rec.k[i])[0], dec(rec.k[i][1]));
                    return out;
                case 'D': out = new g.Date(dec(rec.v)); made[id] = out; return out;
                case 'R': out = new g.RegExp(rec.s, rec.f); made[id] = out; return out;
                case 'M':
                    out = new g.Map(); made[id] = out;
                    list(rec.e);
                    for (i = 0; i < rec.e.length; i++) out.set(dec(list(rec.e[i])[0]), dec(rec.e[i][1]));
                    return out;
                case 'S':
                    out = new g.Set(); made[id] = out;
                    list(rec.e);
                    for (i = 0; i < rec.e.length; i++) out.add(dec(rec.e[i]));
                    return out;
                case 'AB':
                    if (typeof rec.d !== 'string') throw refused();
                    out = unhex(rec.d); made[id] = out; return out;
                case 'TA':
                    if (indexOf(VIEWS, rec.c) < 0) throw refused();
                    out = new g[rec.c](dec(rec.b), rec.o, rec.l); made[id] = out; return out;
                case 'DV': out = new g.DataView(dec(rec.b), rec.o, rec.l); made[id] = out; return out;
                case 'E':
                    if (indexOf(ERROR_NAMES, rec.n) < 0) throw refused();
                    out = new g[rec.n](rec.m); made[id] = out;
                    if (rec.s !== undefined) { try { out.stack = rec.s; } catch (_) {} }
                    return out;
                case 'W': out = Object(dec(rec.v)); made[id] = out; return out;
            }
            throw refused();
        }
        return dec(data.r);
    }

    Object.defineProperty(g, '__rk_sc', {
        value: { serialize: serialize, deserialize: deserialize },
        writable: true, configurable: true, enumerable: false
    });
})(globalThis);
