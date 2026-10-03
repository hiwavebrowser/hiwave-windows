// XMLHttpRequest, built on the script-network bridge (web_net_bridge.js).
//
// This file has no network access of its own: send() queues a request on
// window.__rustkit_net and waits for the engine to deliver the outcome, after
// the request has been vetted and fetched under the page's FetchPolicy (the
// one allow/deny layer). It is installed only after the bridge, so a page the
// engine did not enable the bridge on has no XMLHttpRequest at all.
//
// Stated limits: asynchronous requests only (a synchronous one would have to
// block the engine's pump, so open(..., false) throws NotSupportedError);
// the body arrives whole, so there is one progress event; responseType
// 'document' / responseXML are not supported (the response is null); no
// cookies (the policy sends none).
(function (g) {
    var net = g.__rustkit_net;
    if (!net || typeof g.XMLHttpRequest === 'function') return;

    var UNSENT = 0, OPENED = 1, HEADERS_RECEIVED = 2, LOADING = 3, DONE = 4;
    var S = typeof Symbol === 'function' ? Symbol('xhr') : '__xhr';
    var L = typeof Symbol === 'function' ? Symbol('xhr-listeners') : '__xhrl';

    function domError(message, name) {
        var D = g.DOMException;
        if (typeof D === 'function') return new D(message, name);
        var e = new Error(message); e.name = name; return e;
    }
    function report(e) {
        try { if (g.__rustkit_errors) g.__rustkit_errors.push(String(e)); } catch (_) {}
    }

    // ---- bytes <-> base64
    function toBase64(bytes) {
        var s = '';
        for (var i = 0; i < bytes.length; i += 4096) {
            s += String.fromCharCode.apply(null, bytes.subarray(i, Math.min(i + 4096, bytes.length)));
        }
        return g.btoa(s);
    }
    function fromBase64(b64) {
        var s = g.atob(b64 || ''), out = new Uint8Array(s.length);
        for (var i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
        return out;
    }
    function utf8(str) { return new g.TextEncoder().encode(str); }

    // ---- a small event target (the DOM one is for nodes)
    var TYPES = ['readystatechange', 'loadstart', 'progress', 'abort', 'error', 'load', 'timeout', 'loadend'];
    function initTarget(target) {
        var listeners = {};
        Object.defineProperty(target, L, { value: listeners });
        TYPES.forEach(function (t) { target['on' + t] = null; });
    }
    function listenersOf(target) { return target[L]; }
    function addListener(type, cb, options) {
        if (typeof cb !== 'function' && !(cb && typeof cb.handleEvent === 'function')) return;
        var l = listenersOf(this), list = l[type] || (l[type] = []);
        for (var i = 0; i < list.length; i++) if (list[i].cb === cb) return;
        list.push({ cb: cb, once: !!(options && typeof options === 'object' && options.once) });
    }
    function removeListener(type, cb) {
        var l = listenersOf(this);
        if (l[type]) l[type] = l[type].filter(function (x) { return x.cb !== cb; });
    }
    function dispatch(evt) {
        var self = this;
        try { Object.defineProperty(evt, 'target', { value: self, configurable: true }); } catch (_) {}
        try { Object.defineProperty(evt, 'currentTarget', { value: self, configurable: true }); } catch (_) {}
        var handler = self['on' + evt.type];
        if (typeof handler === 'function') {
            try { handler.call(self, evt); } catch (e) { report(e); }
        }
        (listenersOf(self)[evt.type] || []).slice().forEach(function (x) {
            if (x.once) removeListener.call(self, evt.type, x.cb);
            try { typeof x.cb === 'function' ? x.cb.call(self, evt) : x.cb.handleEvent(evt); } catch (e) { report(e); }
        });
        return true;
    }
    function progressEvent(type, loaded, total) {
        var init = { lengthComputable: total > 0, loaded: loaded, total: total };
        return typeof g.ProgressEvent === 'function' ? new g.ProgressEvent(type, init) : { type: type, loaded: loaded, total: total };
    }
    function fire(target, type, loaded, total) {
        dispatch.call(target, type === 'readystatechange' ? new g.Event(type) : progressEvent(type, loaded || 0, total || 0));
    }

    function XMLHttpRequestEventTarget() { throw new TypeError('Illegal constructor'); }
    XMLHttpRequestEventTarget.prototype.addEventListener = addListener;
    XMLHttpRequestEventTarget.prototype.removeEventListener = removeListener;
    XMLHttpRequestEventTarget.prototype.dispatchEvent = dispatch;

    function XMLHttpRequestUpload() { throw new TypeError('Illegal constructor'); }
    XMLHttpRequestUpload.prototype = Object.create(XMLHttpRequestEventTarget.prototype, {
        constructor: { value: XMLHttpRequestUpload, writable: true, configurable: true }
    });

    // ---- request header rules (the policy enforces these again)
    var FORBIDDEN = {
        'accept-charset': 1, 'accept-encoding': 1, 'access-control-request-headers': 1,
        'access-control-request-method': 1, 'connection': 1, 'content-length': 1, 'cookie': 1,
        'cookie2': 1, 'date': 1, 'dnt': 1, 'expect': 1, 'host': 1, 'keep-alive': 1, 'origin': 1,
        'referer': 1, 'te': 1, 'trailer': 1, 'transfer-encoding': 1, 'upgrade': 1, 'via': 1
    };
    var TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
    var RESPONSE_TYPES = { '': 1, 'text': 1, 'json': 1, 'arraybuffer': 1, 'blob': 1, 'document': 1 };

    function normalizeMethod(method) {
        var m = String(method);
        if (!TOKEN.test(m)) throw domError("Failed to execute 'open' on 'XMLHttpRequest': '" + m + "' is not a valid HTTP method.", 'SyntaxError');
        var up = m.toUpperCase();
        if (up === 'CONNECT' || up === 'TRACE' || up === 'TRACK') {
            throw domError("Failed to execute 'open' on 'XMLHttpRequest': '" + m + "' HTTP method is unsupported.", 'SecurityError');
        }
        return ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'].indexOf(up) >= 0 ? up : m;
    }

    function XMLHttpRequest() {
        if (!(this instanceof XMLHttpRequest)) {
            throw new TypeError("Failed to construct 'XMLHttpRequest': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        }
        initTarget(this);
        var upload = Object.create(XMLHttpRequestUpload.prototype);
        initTarget(upload);
        Object.defineProperty(this, S, {
            value: {
                state: UNSENT, method: 'GET', url: '', headers: [], sendFlag: false, id: 0, upload: upload,
                responseType: '', timeout: 0, withCredentials: false, timer: null, token: 0,
                status: 0, statusText: '', resHeaders: [], bytes: null, responseURL: '', cache: undefined, mime: null
            }
        });
    }
    XMLHttpRequest.prototype = Object.create(XMLHttpRequestEventTarget.prototype, {
        constructor: { value: XMLHttpRequest, writable: true, configurable: true }
    });
    [['UNSENT', 0], ['OPENED', 1], ['HEADERS_RECEIVED', 2], ['LOADING', 3], ['DONE', 4]].forEach(function (c) {
        Object.defineProperty(XMLHttpRequest, c[0], { value: c[1], enumerable: true });
        Object.defineProperty(XMLHttpRequest.prototype, c[0], { value: c[1], enumerable: true });
    });

    function st(x) {
        var s = x != null ? x[S] : undefined;
        if (!s) throw new TypeError('Illegal invocation');
        return s;
    }
    function getter(name, fn) {
        Object.defineProperty(XMLHttpRequest.prototype, name, { get: fn, enumerable: true, configurable: true });
    }
    function setState(x, s, state) {
        if (s.state === state) return;
        s.state = state;
        fire(x, 'readystatechange');
    }

    getter('readyState', function () { return st(this).state; });
    getter('status', function () { return st(this).status; });
    getter('statusText', function () { return st(this).statusText; });
    getter('responseURL', function () { return st(this).responseURL; });
    getter('upload', function () { return st(this).upload; });
    getter('responseXML', function () {
        var s = st(this);
        if (s.responseType !== '' && s.responseType !== 'document') {
            throw domError("Failed to read the 'responseXML' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'document' (was '" + s.responseType + "').", 'InvalidStateError');
        }
        return null;
    });
    Object.defineProperty(XMLHttpRequest.prototype, 'responseType', {
        get: function () { return st(this).responseType; },
        set: function (v) {
            var s = st(this);
            v = String(v);
            if (!RESPONSE_TYPES[v]) return; // an unknown value is ignored
            if (s.state === LOADING || s.state === DONE) {
                throw domError("Failed to set the 'responseType' property on 'XMLHttpRequest': The response type cannot be set if the object's state is LOADING or DONE.", 'InvalidStateError');
            }
            s.responseType = v;
        },
        enumerable: true, configurable: true
    });
    Object.defineProperty(XMLHttpRequest.prototype, 'timeout', {
        get: function () { return st(this).timeout; },
        set: function (v) { st(this).timeout = Math.max(0, Math.floor(Number(v)) || 0); },
        enumerable: true, configurable: true
    });
    Object.defineProperty(XMLHttpRequest.prototype, 'withCredentials', {
        get: function () { return st(this).withCredentials; },
        set: function (v) {
            var s = st(this);
            if ((s.state !== UNSENT && s.state !== OPENED) || s.sendFlag) {
                throw domError("Failed to set the 'withCredentials' property on 'XMLHttpRequest': The value may only be set if the object's state is UNSENT or OPENED.", 'InvalidStateError');
            }
            s.withCredentials = !!v;
        },
        enumerable: true, configurable: true
    });

    function text(s) {
        if (s.cache && s.cache.text !== undefined) return s.cache.text;
        var label = 'utf-8';
        var m = /charset\s*=\s*"?([^";\s]+)/i.exec(s.mime || headerValue(s, 'content-type') || '');
        if (m) label = m[1];
        var t;
        try { t = new g.TextDecoder(label).decode(s.bytes); } catch (_) { t = new g.TextDecoder().decode(s.bytes); }
        (s.cache = s.cache || {}).text = t;
        return t;
    }
    getter('responseText', function () {
        var s = st(this);
        if (s.responseType !== '' && s.responseType !== 'text') {
            throw domError("Failed to read the 'responseText' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'text' (was '" + s.responseType + "').", 'InvalidStateError');
        }
        if (s.state !== LOADING && s.state !== DONE) return '';
        return s.bytes ? text(s) : '';
    });
    getter('response', function () {
        var s = st(this), t = s.responseType;
        if (t === '' || t === 'text') {
            if (s.state !== LOADING && s.state !== DONE) return '';
            return s.bytes ? text(s) : '';
        }
        if (s.state !== DONE || !s.bytes) return null;
        if (s.cache && s.cache.value !== undefined) return s.cache.value;
        var v;
        if (t === 'json') { try { v = JSON.parse(text(s)); } catch (_) { v = null; } }
        else if (t === 'arraybuffer') { v = s.bytes.buffer.slice(s.bytes.byteOffset, s.bytes.byteOffset + s.bytes.byteLength); }
        else if (t === 'blob') { v = new g.Blob([s.bytes], { type: s.mime || headerValue(s, 'content-type') || '' }); }
        else { v = null; }
        (s.cache = s.cache || {}).value = v;
        return v;
    });

    function headerValue(s, name) {
        var out = [];
        name = String(name).toLowerCase();
        for (var i = 0; i < s.resHeaders.length; i++) if (s.resHeaders[i][0] === name) out.push(s.resHeaders[i][1]);
        return out.length ? out.join(', ') : null;
    }

    XMLHttpRequest.prototype.open = function (method, url, async, user, password) {
        var s = st(this);
        if (arguments.length < 2) {
            throw new TypeError("Failed to execute 'open' on 'XMLHttpRequest': 2 arguments required, but only " + arguments.length + ' present.');
        }
        method = normalizeMethod(method);
        var abs;
        try { abs = new g.URL(String(url), g.document && g.document.baseURI ? g.document.baseURI : g.location.href).href; }
        catch (_) { throw domError("Failed to execute 'open' on 'XMLHttpRequest': Invalid URL", 'SyntaxError'); }
        if (async !== undefined && !async) {
            throw domError("Failed to execute 'open' on 'XMLHttpRequest': synchronous requests are not supported", 'NotSupportedError');
        }
        // open() restarts: abandon anything in flight without events.
        endRequest(s);
        s.method = method; s.url = abs.split('#')[0]; s.headers = []; s.sendFlag = false;
        s.status = 0; s.statusText = ''; s.resHeaders = []; s.bytes = null; s.responseURL = ''; s.cache = undefined; s.mime = null;
        s.state = UNSENT;
        setState(this, s, OPENED);
    };

    XMLHttpRequest.prototype.setRequestHeader = function (name, value) {
        var s = st(this);
        if (arguments.length < 2) {
            throw new TypeError("Failed to execute 'setRequestHeader' on 'XMLHttpRequest': 2 arguments required, but only " + arguments.length + ' present.');
        }
        if (s.state !== OPENED || s.sendFlag) {
            throw domError("Failed to execute 'setRequestHeader' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError');
        }
        name = String(name); value = String(value).replace(/^[ \t]+|[ \t]+$/g, '');
        if (!TOKEN.test(name) || /[\r\n\0]/.test(value)) {
            throw domError("Failed to execute 'setRequestHeader' on 'XMLHttpRequest': '" + name + "' is not a valid HTTP header field name.", 'SyntaxError');
        }
        var lower = name.toLowerCase();
        if (FORBIDDEN[lower] || lower.indexOf('proxy-') === 0 || lower.indexOf('sec-') === 0) return; // refused, silently
        for (var i = 0; i < s.headers.length; i++) {
            if (s.headers[i][0].toLowerCase() === lower) { s.headers[i][1] += ', ' + value; return; }
        }
        s.headers.push([name, value]);
    };

    XMLHttpRequest.prototype.overrideMimeType = function (mime) {
        var s = st(this);
        if (s.state === LOADING || s.state === DONE) {
            throw domError("Failed to execute 'overrideMimeType' on 'XMLHttpRequest': MIME types cannot be overridden when the state is LOADING or DONE.", 'InvalidStateError');
        }
        s.mime = String(mime);
    };

    function hasHeader(s, lower) {
        for (var i = 0; i < s.headers.length; i++) if (s.headers[i][0].toLowerCase() === lower) return true;
        return false;
    }

    // The request body as bytes (and the Content-Type to default to), or a
    // promise of them when they are not at hand.
    function encodeBody(body) {
        if (body === null || body === undefined) return null;
        if (typeof body === 'string') return { bytes: utf8(body), type: 'text/plain;charset=UTF-8' };
        if (typeof g.URLSearchParams === 'function' && body instanceof g.URLSearchParams) {
            return { bytes: utf8(body.toString()), type: 'application/x-www-form-urlencoded;charset=UTF-8' };
        }
        if (body instanceof g.ArrayBuffer) return { bytes: new Uint8Array(body.slice(0)), type: null };
        if (g.ArrayBuffer.isView(body)) {
            return { bytes: new Uint8Array(body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength)), type: null };
        }
        if (typeof g.Blob === 'function' && body instanceof g.Blob) {
            return body.arrayBuffer().then(function (buf) { return { bytes: new Uint8Array(buf), type: body.type || null }; });
        }
        if (typeof g.FormData === 'function' && body instanceof g.FormData) return encodeForm(body);
        return { bytes: utf8(String(body)), type: 'text/plain;charset=UTF-8' };
    }
    function encodeForm(form) {
        var boundary = '----RustKitFormBoundary' + Math.random().toString(36).slice(2, 12) + Date.now().toString(36);
        var entries = [];
        form.forEach(function (value, name) { entries.push([name, value]); });
        var chunks = [];
        function esc(v) { return String(v).replace(/\r\n|\r|\n/g, '%0D%0A').replace(/"/g, '%22'); }
        var pending = entries.map(function (e) {
            var name = e[0], value = e[1];
            if (typeof g.Blob === 'function' && value instanceof g.Blob) {
                return value.arrayBuffer().then(function (buf) {
                    var fname = value.name !== undefined ? value.name : 'blob';
                    return { head: '--' + boundary + '\r\nContent-Disposition: form-data; name="' + esc(name) + '"; filename="' + esc(fname) + '"\r\nContent-Type: ' + (value.type || 'application/octet-stream') + '\r\n\r\n', data: new Uint8Array(buf) };
                });
            }
            return Promise.resolve({ head: '--' + boundary + '\r\nContent-Disposition: form-data; name="' + esc(name) + '"\r\n\r\n', data: utf8(String(value).replace(/\r\n|\r|\n/g, '\r\n')) });
        });
        return Promise.all(pending).then(function (parts) {
            parts.forEach(function (p) { chunks.push(utf8(p.head), p.data, utf8('\r\n')); });
            chunks.push(utf8('--' + boundary + '--\r\n'));
            var total = 0; chunks.forEach(function (c) { total += c.length; });
            var bytes = new Uint8Array(total), at = 0;
            chunks.forEach(function (c) { bytes.set(c, at); at += c.length; });
            return { bytes: bytes, type: 'multipart/form-data; boundary=' + boundary };
        });
    }

    function endRequest(s) {
        s.token++; // any delivery still to come is for an abandoned request
        if (s.id) { net.cancel(s.id); s.id = 0; }
        if (s.timer !== null) { g.clearTimeout(s.timer); s.timer = null; }
        s.sendFlag = false;
    }

    XMLHttpRequest.prototype.send = function (body) {
        var x = this, s = st(x);
        if (s.state !== OPENED) {
            throw domError("Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError');
        }
        if (s.sendFlag) {
            throw domError("Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError');
        }
        if (s.method === 'GET' || s.method === 'HEAD') body = null;
        var encoded = encodeBody(body);
        s.sendFlag = true;
        var token = ++s.token;
        var hasBody = encoded !== null;
        fire(x, 'loadstart', 0, 0);
        if (hasBody) fire(s.upload, 'loadstart', 0, 0);
        if (s.timeout > 0) {
            s.timer = g.setTimeout(function () {
                if (s.token !== token) return;
                s.timer = null;
                finish(x, s, 'timeout', hasBody);
            }, s.timeout);
        }
        Promise.resolve(encoded).then(function (enc) {
            if (s.token !== token) return; // aborted or reopened meanwhile
            var headers = s.headers.map(function (h) { return [h[0], h[1]]; });
            if (enc && enc.type && !hasHeader(s, 'content-type')) headers.push(['Content-Type', enc.type]);
            var id = net.request({
                method: s.method, url: s.url, headers: headers,
                body_b64: enc ? toBase64(enc.bytes) : null,
                mode: 'cors', credentials: s.withCredentials ? 'include' : 'same-origin',
                redirect: 'follow', destination: 'xhr'
            }, function (r) {
                if (s.token !== token) return;
                s.id = 0;
                if (!r.ok) { finish(x, s, 'error', hasBody); return; }
                deliver(x, s, r, hasBody);
            });
            if (id === 0) { finish(x, s, 'error', hasBody); return; } // the queue is full
            s.id = id;
        }, function () {
            if (s.token === token) finish(x, s, 'error', hasBody);
        });
    };

    function deliver(x, s, r, hasBody) {
        var token = s.token;
        s.status = r.status;
        s.statusText = r.status_text || '';
        s.responseURL = r.url || s.url;
        s.resHeaders = (r.headers || []).map(function (h) { return [String(h[0]).toLowerCase(), String(h[1])]; });
        s.bytes = fromBase64(r.body_b64);
        s.cache = undefined;
        if (r.kind === 'opaque' || r.kind === 'opaqueredirect') { s.status = 0; s.statusText = ''; s.resHeaders = []; s.bytes = new Uint8Array(0); }
        if (hasBody) {
            fire(s.upload, 'progress', 1, 1);
            fire(s.upload, 'load', 1, 1);
            fire(s.upload, 'loadend', 1, 1);
            if (s.token !== token) return;
        }
        // A handler may abort() or reopen the request between any two of
        // these steps; each step checks.
        setState(x, s, HEADERS_RECEIVED);
        if (s.token !== token) return;
        setState(x, s, LOADING);
        if (s.token !== token) return;
        fire(x, 'progress', s.bytes.length, s.bytes.length);
        if (s.token !== token) return;
        s.sendFlag = false;
        if (s.timer !== null) { g.clearTimeout(s.timer); s.timer = null; }
        setState(x, s, DONE);
        if (s.token !== token) return;
        fire(x, 'load', s.bytes.length, s.bytes.length);
        if (s.token !== token) return;
        fire(x, 'loadend', s.bytes.length, s.bytes.length);
    }

    // The request ended without a response: a network error or a timeout.
    function finish(x, s, kind, hasBody) {
        if (s.id) { net.cancel(s.id); s.id = 0; }
        if (s.timer !== null) { g.clearTimeout(s.timer); s.timer = null; }
        s.token++;
        s.sendFlag = false;
        s.status = 0; s.statusText = ''; s.resHeaders = []; s.bytes = null; s.responseURL = ''; s.cache = undefined;
        setState(x, s, DONE);
        if (hasBody) {
            fire(s.upload, kind, 0, 0);
            fire(s.upload, 'loadend', 0, 0);
        }
        fire(x, kind, 0, 0);
        fire(x, 'loadend', 0, 0);
    }

    XMLHttpRequest.prototype.abort = function () {
        var x = this, s = st(x);
        var wasActive = s.sendFlag && (s.state === OPENED || s.state === HEADERS_RECEIVED || s.state === LOADING);
        endRequest(s);
        if (wasActive) {
            s.status = 0; s.statusText = ''; s.resHeaders = []; s.bytes = null; s.responseURL = ''; s.cache = undefined;
            setState(x, s, DONE);
            fire(x, 'abort', 0, 0);
            fire(x, 'loadend', 0, 0);
        }
        if (s.state === DONE) s.state = UNSENT; // back to UNSENT, without an event
    };

    XMLHttpRequest.prototype.getResponseHeader = function (name) {
        var s = st(this);
        if (arguments.length < 1) {
            throw new TypeError("Failed to execute 'getResponseHeader' on 'XMLHttpRequest': 1 argument required, but only 0 present.");
        }
        if (s.state < HEADERS_RECEIVED) return null;
        return headerValue(s, name);
    };
    XMLHttpRequest.prototype.getAllResponseHeaders = function () {
        var s = st(this);
        if (s.state < HEADERS_RECEIVED) return '';
        var names = [];
        s.resHeaders.forEach(function (h) { if (names.indexOf(h[0]) < 0) names.push(h[0]); });
        names.sort();
        return names.map(function (n) { return n + ': ' + headerValue(s, n) + '\r\n'; }).join('');
    };
    Object.defineProperty(XMLHttpRequest.prototype, Symbol.toStringTag, { value: 'XMLHttpRequest', configurable: true });

    function def(name, value) {
        Object.defineProperty(g, name, { value: value, writable: true, configurable: true, enumerable: false });
    }
    def('XMLHttpRequestEventTarget', XMLHttpRequestEventTarget);
    def('XMLHttpRequestUpload', XMLHttpRequestUpload);
    def('XMLHttpRequest', XMLHttpRequest);
})(typeof globalThis === 'object' ? globalThis : this);
