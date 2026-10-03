// fetch, Headers, Request and Response, built on the script-network bridge.
//
// No network access of its own: fetch() queues a request on
// window.__rustkit_net and waits for the engine, which vets and fetches it
// under the page's FetchPolicy (the one allow/deny layer). Installed only
// after the bridge and XMLHttpRequest (which shares its body encoding).
//
// Stated limits: the body arrives whole, so `Response.body` is a stream of
// one chunk; `formData()` parses urlencoded bodies only; `Request.body`
// streams are read whole before sending; no cookies (the policy sends none);
// `cache`, `integrity`, `keepalive`, `priority` are stored and ignored.
(function (g) {
    var net = g.__rustkit_net;
    if (!net || !net.util || typeof g.fetch === 'function') return;
    var U = net.util;
    var domError = U.domError, utf8 = U.utf8;

    var TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
    var FORBIDDEN_REQUEST = {
        'accept-charset': 1, 'accept-encoding': 1, 'access-control-request-headers': 1,
        'access-control-request-method': 1, 'connection': 1, 'content-length': 1, 'cookie': 1,
        'cookie2': 1, 'date': 1, 'dnt': 1, 'expect': 1, 'host': 1, 'keep-alive': 1, 'origin': 1,
        'referer': 1, 'te': 1, 'trailer': 1, 'transfer-encoding': 1, 'upgrade': 1, 'via': 1
    };
    var H = typeof Symbol === 'function' ? Symbol('headers') : '__headers';
    var B = typeof Symbol === 'function' ? Symbol('body') : '__body';
    var R = typeof Symbol === 'function' ? Symbol('request') : '__request';
    var P = typeof Symbol === 'function' ? Symbol('response') : '__response';

    function typeError(m) { return new TypeError(m); }
    function def(name, value) {
        Object.defineProperty(g, name, { value: value, writable: true, configurable: true, enumerable: false });
    }
    function method(proto, name, fn) {
        Object.defineProperty(proto, name, { value: fn, writable: true, configurable: true, enumerable: true });
    }
    function getter(proto, name, fn) {
        Object.defineProperty(proto, name, { get: fn, enumerable: true, configurable: true });
    }

    // ---- Headers (Fetch §5.2)
    function normalizeValue(v) { return String(v).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, ''); }
    function checkName(name, who) {
        name = String(name);
        if (!TOKEN.test(name)) throw typeError("Failed to execute '" + who + "' on 'Headers': Invalid name");
        return name.toLowerCase();
    }
    function checkValue(value, who) {
        value = normalizeValue(value);
        if (/[\0\r\n]/.test(value)) throw typeError("Failed to execute '" + who + "' on 'Headers': Invalid value");
        return value;
    }
    function Headers(init) {
        if (!(this instanceof Headers)) throw typeError("Failed to construct 'Headers': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        Object.defineProperty(this, H, { value: { list: [], guard: 'none' }, writable: true });
        if (init === undefined || init === null) return;
        fill(this, init);
    }
    function fill(h, init) {
        if (init instanceof Headers) {
            init[H].list.forEach(function (e) { h.append(e[0], e[1]); });
        } else if (typeof init === 'object' && typeof init[Symbol.iterator] === 'function') {
            Array.from(init).forEach(function (pair) {
                var p = Array.from(pair);
                if (p.length !== 2) throw typeError("Failed to construct 'Headers': Invalid value");
                h.append(p[0], p[1]);
            });
        } else if (typeof init === 'object') {
            Object.keys(init).forEach(function (k) { h.append(k, init[k]); });
        } else {
            throw typeError("Failed to construct 'Headers': The provided value is not of type '(record<ByteString, ByteString> or sequence<sequence<ByteString>>)'.");
        }
    }
    // CORS-safelisted request headers (Fetch §2.2.2), the only ones a
    // `no-cors` request may carry, however the headers object is mutated
    // after the request is built.
    var SAFELISTED = { 'accept': 1, 'accept-language': 1, 'content-language': 1, 'content-type': 1 };
    function safelisted(name, value) {
        if (!SAFELISTED[name]) return false;
        if (value.length > 128) return false;
        if (name === 'content-type') {
            var essence = value.split(';')[0].replace(/^[	 ]+|[	 ]+$/g, '').toLowerCase();
            return essence === 'application/x-www-form-urlencoded' || essence === 'multipart/form-data' || essence === 'text/plain';
        }
        return true;
    }
    function guarded(h, name, value) {
        var s = h[H];
        if (s.guard === 'immutable') throw typeError('Headers are immutable');
        if (s.guard === 'request-no-cors') return safelisted(name, value === undefined ? '' : value);
        if (s.guard === 'request' && (FORBIDDEN_REQUEST[name] || name.indexOf('proxy-') === 0 || name.indexOf('sec-') === 0)) return false;
        if (s.guard === 'response' && (name === 'set-cookie' || name === 'set-cookie2')) return false;
        return true;
    }
    method(Headers.prototype, 'append', function (name, value) {
        if (arguments.length < 2) throw typeError("Failed to execute 'append' on 'Headers': 2 arguments required, but only " + arguments.length + ' present.');
        var n = checkName(name, 'append'), v = checkValue(value, 'append');
        if (!guarded(this, n, v)) return;
        this[H].list.push([n, v]);
    });
    method(Headers.prototype, 'set', function (name, value) {
        if (arguments.length < 2) throw typeError("Failed to execute 'set' on 'Headers': 2 arguments required, but only " + arguments.length + ' present.');
        var n = checkName(name, 'set'), v = checkValue(value, 'set');
        if (!guarded(this, n, v)) return;
        var list = this[H].list, at = -1;
        for (var i = 0; i < list.length; i++) if (list[i][0] === n) { at = i; break; }
        if (at < 0) { list.push([n, v]); return; }
        list[at][1] = v;
        this[H].list = list.filter(function (e, i2) { return e[0] !== n || i2 === at; });
    });
    method(Headers.prototype, 'delete', function (name) {
        var n = checkName(name, 'delete');
        if (!guarded(this, n)) return;
        this[H].list = this[H].list.filter(function (e) { return e[0] !== n; });
    });
    method(Headers.prototype, 'get', function (name) {
        var n = checkName(name, 'get'), out = [];
        this[H].list.forEach(function (e) { if (e[0] === n) out.push(e[1]); });
        return out.length ? out.join(', ') : null;
    });
    method(Headers.prototype, 'has', function (name) {
        var n = checkName(name, 'has');
        return this[H].list.some(function (e) { return e[0] === n; });
    });
    method(Headers.prototype, 'getSetCookie', function () {
        return this[H].list.filter(function (e) { return e[0] === 'set-cookie'; }).map(function (e) { return e[1]; });
    });
    // Sorted by name, equal names combined; each Set-Cookie on its own.
    function entriesOf(h) {
        var names = [], combined = {}, out = [];
        h[H].list.forEach(function (e) {
            if (e[0] === 'set-cookie') { out.push([e[0], e[1]]); return; }
            if (!(e[0] in combined)) { names.push(e[0]); combined[e[0]] = []; }
            combined[e[0]].push(e[1]);
        });
        names.forEach(function (n) { out.push([n, combined[n].join(', ')]); });
        out.sort(function (a, b) { return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0; });
        return out;
    }
    method(Headers.prototype, 'forEach', function (cb, thisArg) {
        if (typeof cb !== 'function') throw typeError("Failed to execute 'forEach' on 'Headers': parameter 1 is not of type 'Function'.");
        var self = this;
        entriesOf(this).forEach(function (e) { cb.call(thisArg, e[1], e[0], self); });
    });
    function iterator(items) {
        var i = 0;
        var it = {
            next: function () { return i < items.length ? { value: items[i++], done: false } : { value: undefined, done: true }; }
        };
        it[Symbol.iterator] = function () { return it; };
        return it;
    }
    method(Headers.prototype, 'entries', function () { return iterator(entriesOf(this)); });
    method(Headers.prototype, 'keys', function () { return iterator(entriesOf(this).map(function (e) { return e[0]; })); });
    method(Headers.prototype, 'values', function () { return iterator(entriesOf(this).map(function (e) { return e[1]; })); });
    Object.defineProperty(Headers.prototype, Symbol.iterator, { value: Headers.prototype.entries, writable: true, configurable: true });
    Object.defineProperty(Headers.prototype, Symbol.toStringTag, { value: 'Headers', configurable: true });
    function guardedHeaders(guard, init) {
        var h = new Headers();
        h[H].guard = guard;
        if (init !== undefined && init !== null) fill(h, init);
        return h;
    }

    // ---- Body mixin (Fetch §6.2)
    var NULL_BODY_STATUS = { 101: 1, 204: 1, 205: 1, 304: 1 };
    function extract(init) {
        // -> { bytes, type } | promise of it | null
        if (init === null || init === undefined) return null;
        if (typeof g.ReadableStream === 'function' && init instanceof g.ReadableStream) {
            if (init.locked) throw typeError('The body stream is locked');
            var reader = init.getReader(), chunks = [], total = 0;
            var pump = function () {
                return reader.read().then(function (r) {
                    if (r.done) {
                        var bytes = new Uint8Array(total), at = 0;
                        chunks.forEach(function (c) { bytes.set(c, at); at += c.length; });
                        return { bytes: bytes, type: null };
                    }
                    var c = r.value instanceof Uint8Array ? r.value : (typeof r.value === 'string' ? utf8(r.value) : new Uint8Array(r.value));
                    chunks.push(c); total += c.length;
                    return pump();
                });
            };
            return pump();
        }
        return U.encodeBody(init);
    }
    function newBody(extracted) {
        // extracted may be a promise: the body then resolves later.
        var b = { bytes: null, type: null, used: false, ready: null, stream: null };
        if (extracted && typeof extracted.then === 'function') {
            b.ready = extracted.then(function (e) { b.bytes = e.bytes; b.type = e.type; b.ready = null; });
        } else if (extracted) {
            b.bytes = extracted.bytes; b.type = extracted.type;
        }
        return b;
    }
    function consume(obj, how) {
        var b = obj[B], who = obj instanceof Request ? 'Request' : 'Response';
        if (b.used) return Promise.reject(typeError("Failed to execute '" + how + "' on '" + who + "': body stream already read"));
        b.used = true;
        return Promise.resolve(b.ready).then(function () {
            var bytes = b.bytes || new Uint8Array(0);
            if (how === 'arrayBuffer') return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
            if (how === 'bytes') return new Uint8Array(bytes);
            if (how === 'blob') return new g.Blob([bytes], { type: contentType(obj) || '' });
            var text = new g.TextDecoder().decode(bytes);
            if (how === 'text') return text;
            if (how === 'json') return JSON.parse(text);
            if (how === 'formData') {
                var type = contentType(obj) || '';
                if (type.toLowerCase().indexOf('application/x-www-form-urlencoded') !== 0) {
                    throw typeError("Failed to execute 'formData' on '" + who + "': only application/x-www-form-urlencoded bodies are supported");
                }
                var fd = new g.FormData();
                new g.URLSearchParams(text).forEach(function (v, k) { fd.append(k, v); });
                return fd;
            }
        });
    }
    function contentType(obj) {
        var h = obj.headers;
        return h ? h.get('content-type') : null;
    }
    function mixin(proto) {
        ['arrayBuffer', 'blob', 'bytes', 'formData', 'json', 'text'].forEach(function (how) {
            method(proto, how, function () { return consume(this, how); });
        });
        getter(proto, 'bodyUsed', function () { return this[B].used; });
        getter(proto, 'body', function () {
            var b = this[B];
            if (b.bytes === null && !b.ready) return null;
            if (!b.stream) {
                var done = false;
                b.stream = new g.ReadableStream({
                    pull: function (controller) {
                        if (done) return;
                        done = true;
                        b.used = true;
                        return Promise.resolve(b.ready).then(function () {
                            if (b.bytes && b.bytes.length) controller.enqueue(new Uint8Array(b.bytes));
                            controller.close();
                        });
                    }
                }, { highWaterMark: 0 });
            }
            return b.stream;
        });
    }

    // ---- Request (Fetch §5.3)
    var MODES = { 'cors': 1, 'no-cors': 1, 'same-origin': 1, 'navigate': 1 };
    var CREDENTIALS = { 'omit': 1, 'same-origin': 1, 'include': 1 };
    var REDIRECTS = { 'follow': 1, 'error': 1, 'manual': 1 };
    function normalizeMethod(m) {
        m = String(m);
        if (!TOKEN.test(m)) throw typeError("Failed to construct 'Request': '" + m + "' is not a valid HTTP method.");
        var up = m.toUpperCase();
        if (up === 'CONNECT' || up === 'TRACE' || up === 'TRACK') throw typeError("Failed to construct 'Request': '" + m + "' HTTP method is unsupported.");
        return ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'].indexOf(up) >= 0 ? up : m;
    }
    function resolveUrl(input) {
        try {
            var u = new g.URL(String(input), g.document && g.document.baseURI ? g.document.baseURI : g.location.href);
            if (u.username || u.password) throw 0;
            return u.href;
        } catch (_) {
            throw typeError("Failed to construct 'Request': Failed to parse URL from " + String(input));
        }
    }
    function Request(input, init) {
        if (!(this instanceof Request)) throw typeError("Failed to construct 'Request': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        if (arguments.length < 1) throw typeError("Failed to construct 'Request': 1 argument required, but only 0 present.");
        init = init || {};
        var s = { url: '', method: 'GET', mode: 'cors', credentials: 'same-origin', redirect: 'follow', cache: 'default', referrer: 'about:client', referrerPolicy: '', integrity: '', keepalive: false, signal: null };
        var inputBody = null, inputHeaders = null;
        if (input instanceof Request) {
            var o = input[R];
            Object.keys(s).forEach(function (k) { s[k] = o[k]; });
            inputHeaders = input.headers;
            if (input[B].used) throw typeError("Failed to construct 'Request': Cannot construct a Request with a Request object that has already been used.");
            inputBody = input[B];
        } else {
            s.url = resolveUrl(input);
        }
        if (init.method !== undefined) s.method = normalizeMethod(init.method);
        if (init.mode !== undefined) {
            if (!MODES[init.mode]) throw typeError("Failed to construct 'Request': The provided value '" + init.mode + "' is not a valid enum value of type RequestMode.");
            if (init.mode === 'navigate') throw typeError("Failed to construct 'Request': Cannot construct a Request with a RequestInit whose mode member is set as 'navigate'.");
            s.mode = init.mode;
        }
        if (init.credentials !== undefined) {
            if (!CREDENTIALS[init.credentials]) throw typeError("Failed to construct 'Request': The provided value '" + init.credentials + "' is not a valid enum value of type RequestCredentials.");
            s.credentials = init.credentials;
        }
        if (init.redirect !== undefined) {
            if (!REDIRECTS[init.redirect]) throw typeError("Failed to construct 'Request': The provided value '" + init.redirect + "' is not a valid enum value of type RequestRedirect.");
            s.redirect = init.redirect;
        }
        ['cache', 'referrer', 'referrerPolicy', 'integrity'].forEach(function (k) { if (init[k] !== undefined) s[k] = String(init[k]); });
        if (init.keepalive !== undefined) s.keepalive = !!init.keepalive;
        if (init.signal !== undefined) {
            if (init.signal !== null && !(g.AbortSignal && init.signal instanceof g.AbortSignal)) throw typeError("Failed to construct 'Request': member signal is not of type AbortSignal.");
            s.signal = init.signal;
        } else if (input instanceof Request) {
            s.signal = input[R].signal;
        }
        Object.defineProperty(this, R, { value: s });
        if (s.mode === 'no-cors' && ['GET', 'HEAD', 'POST'].indexOf(s.method) < 0) {
            throw typeError("Failed to construct 'Request': '" + s.method + "' is unsupported in no-cors mode.");
        }
        // The guard stays with the headers object for as long as the request
        // lives: a no-cors request can never be widened by a later
        // append()/set().
        var headers = guardedHeaders(s.mode === 'no-cors' ? 'request-no-cors' : 'request', init.headers !== undefined ? init.headers : inputHeaders);
        Object.defineProperty(this, H, { value: headers });
        var hasInitBody = init.body !== undefined && init.body !== null;
        if ((hasInitBody || (inputBody && (inputBody.bytes !== null || inputBody.ready))) && (s.method === 'GET' || s.method === 'HEAD')) {
            throw typeError("Failed to construct 'Request': Request with GET/HEAD method cannot have body.");
        }
        var body;
        if (hasInitBody) {
            body = newBody(extract(init.body));
            if (body.type && !headers.has('content-type')) headers.append('content-type', body.type);
        } else if (inputBody) {
            body = { bytes: inputBody.bytes, type: inputBody.type, used: false, ready: null, stream: null };
            if (inputBody.ready) {
                // The source body is still being read: the copy follows it.
                body.ready = inputBody.ready.then(function () { body.bytes = inputBody.bytes; body.type = inputBody.type; body.ready = null; });
            }
            inputBody.used = true; // the body moves to the new request
        } else {
            body = newBody(null);
        }
        Object.defineProperty(this, B, { value: body });
    }
    getter(Request.prototype, 'url', function () { return this[R].url; });
    getter(Request.prototype, 'method', function () { return this[R].method; });
    getter(Request.prototype, 'headers', function () { return this[H]; });
    ['mode', 'credentials', 'redirect', 'cache', 'referrer', 'referrerPolicy', 'integrity', 'keepalive'].forEach(function (k) {
        getter(Request.prototype, k, function () { return this[R][k]; });
    });
    getter(Request.prototype, 'destination', function () { return ''; });
    getter(Request.prototype, 'signal', function () { return this[R].signal; });
    mixin(Request.prototype);
    method(Request.prototype, 'clone', function () {
        if (this[B].used) throw typeError("Failed to execute 'clone' on 'Request': Request body is already used");
        var copy = new Request(this);
        this[B].used = false; // a clone shares the bytes; the original stays readable
        return copy;
    });
    Object.defineProperty(Request.prototype, Symbol.toStringTag, { value: 'Request', configurable: true });

    // ---- Response (Fetch §5.5)
    function makeResponse(bodyBytes, type, status, statusText, headers, url, redirected) {
        var r = Object.create(Response.prototype);
        Object.defineProperty(r, P, { value: { type: type, status: status, statusText: statusText, url: url || '', redirected: !!redirected } });
        Object.defineProperty(r, H, { value: headers });
        Object.defineProperty(r, B, { value: { bytes: bodyBytes, type: null, used: false, ready: null, stream: null } });
        return r;
    }
    function Response(body, init) {
        if (!(this instanceof Response)) throw typeError("Failed to construct 'Response': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        init = init || {};
        var status = init.status === undefined ? 200 : Number(init.status);
        if (!(status >= 200 && status <= 599) || Math.floor(status) !== status) {
            throw new RangeError("Failed to construct 'Response': The status provided (" + init.status + ') is outside the range [200, 599].');
        }
        var statusText = init.statusText === undefined ? '' : String(init.statusText);
        if (/[\r\n]/.test(statusText)) throw typeError("Failed to construct 'Response': Invalid statusText");
        var headers = guardedHeaders('response', init.headers);
        var extracted = (body === undefined || body === null) ? null : extract(body);
        if (extracted && NULL_BODY_STATUS[status]) throw typeError("Failed to construct 'Response': Response with null body status cannot have body");
        var b = newBody(extracted);
        if (b.type && !headers.has('content-type')) headers.append('content-type', b.type);
        Object.defineProperty(this, P, { value: { type: 'default', status: status, statusText: statusText, url: '', redirected: false } });
        Object.defineProperty(this, H, { value: headers });
        Object.defineProperty(this, B, { value: b });
    }
    getter(Response.prototype, 'type', function () { return this[P].type; });
    getter(Response.prototype, 'status', function () { return this[P].status; });
    getter(Response.prototype, 'ok', function () { return this[P].status >= 200 && this[P].status <= 299; });
    getter(Response.prototype, 'statusText', function () { return this[P].statusText; });
    getter(Response.prototype, 'headers', function () { return this[H]; });
    getter(Response.prototype, 'url', function () { return this[P].url; });
    getter(Response.prototype, 'redirected', function () { return this[P].redirected; });
    mixin(Response.prototype);
    method(Response.prototype, 'clone', function () {
        if (this[B].used) throw typeError("Failed to execute 'clone' on 'Response': Response body is already used");
        var c = makeResponse(this[B].bytes ? new Uint8Array(this[B].bytes) : null, this[P].type, this[P].status, this[P].statusText, cloneHeaders(this[H]), this[P].url, this[P].redirected);
        var source = this[B];
        if (source.ready) { c[B].ready = source.ready.then(function () { c[B].bytes = source.bytes ? new Uint8Array(source.bytes) : null; c[B].ready = null; }); }
        return c;
    });
    function cloneHeaders(h) {
        var n = new Headers();
        n[H].list = h[H].list.map(function (e) { return [e[0], e[1]]; });
        n[H].guard = h[H].guard;
        return n;
    }
    Response.error = function () {
        return makeResponse(null, 'error', 0, '', (function () { var h = new Headers(); h[H].guard = 'immutable'; return h; })(), '', false);
    };
    Response.redirect = function (url, status) {
        status = status === undefined ? 302 : Number(status);
        if ([301, 302, 303, 307, 308].indexOf(status) < 0) throw new RangeError("Failed to execute 'redirect' on 'Response': Invalid status code");
        var r = new Response(null, { status: 200 });
        r[P].status = status;
        r[H].append('location', new g.URL(String(url), g.location.href).href);
        r[H][H].guard = 'immutable';
        return r;
    };
    Response.json = function (data, init) {
        var text = JSON.stringify(data);
        if (text === undefined) throw typeError("Failed to execute 'json' on 'Response': The data is not JSON serializable");
        var headers = new Headers(init && init.headers);
        if (!headers.has('content-type')) headers.set('content-type', 'application/json');
        var copy = {};
        if (init) Object.keys(init).forEach(function (k) { copy[k] = init[k]; });
        copy.headers = headers;
        return new Response(text, copy);
    };
    Object.defineProperty(Response.prototype, Symbol.toStringTag, { value: 'Response', configurable: true });

    // ---- fetch (Fetch §5.6)
    var KIND_TYPE = { basic: 'basic', cors: 'cors', opaque: 'opaque', opaqueredirect: 'opaqueredirect' };
    function abortReason(signal) {
        return signal.reason !== undefined ? signal.reason : domError('signal is aborted without reason', 'AbortError');
    }
    function fetch(input, init) {
        return new Promise(function (resolve, reject) {
            var req;
            try { req = new Request(input, init); } catch (e) { reject(e); return; }
            var signal = req[R].signal;
            if (signal && signal.aborted) { reject(abortReason(signal)); return; }
            var s = req[R], body = req[B];
            body.used = true;
            var settled = false, id = 0, onAbort = null;
            function done() {
                settled = true;
                if (signal && onAbort) signal.removeEventListener('abort', onAbort);
            }
            if (signal) {
                onAbort = function () {
                    if (settled) return;
                    done();
                    if (id) net.cancel(id);
                    reject(abortReason(signal));
                };
                signal.addEventListener('abort', onAbort);
            }
            Promise.resolve(body.ready).then(function () {
                if (settled) return;
                var headers = req[H][H].list.map(function (e) { return [e[0], e[1]]; });
                id = net.request({
                    method: s.method, url: s.url, headers: headers,
                    body_b64: body.bytes && body.bytes.length ? U.toBase64(body.bytes) : (body.bytes ? '' : null),
                    mode: s.mode, credentials: s.credentials, redirect: s.redirect, destination: 'fetch'
                }, function (r) {
                    if (settled) return;
                    done();
                    if (!r.ok) { reject(typeError('Failed to fetch')); return; }
                    var type = KIND_TYPE[r.kind] || 'basic';
                    var h = new Headers();
                    (r.headers || []).forEach(function (e) { h[H].list.push([String(e[0]).toLowerCase(), String(e[1])]); });
                    h[H].guard = 'immutable';
                    var status = r.status, bytes = U.fromBase64(r.body_b64);
                    if (type === 'opaque' || type === 'opaqueredirect') { status = 0; h[H].list = []; bytes = null; }
                    if (NULL_BODY_STATUS[status]) bytes = null;
                    resolve(makeResponse(bytes, type, status, type === 'opaque' || type === 'opaqueredirect' ? '' : (r.status_text || ''), h, type === 'opaque' ? '' : r.url, !!r.redirected));
                });
                if (id === 0) { done(); reject(typeError('Failed to fetch')); }
            }, function (e) { if (!settled) { done(); reject(e); } });
        });
    }

    def('Headers', Headers);
    def('Request', Request);
    def('Response', Response);
    def('fetch', fetch);
})(typeof globalThis === 'object' ? globalThis : this);
