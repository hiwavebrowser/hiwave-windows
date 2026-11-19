// The page side of dedicated workers (STR-4). `new Worker(url)` fetches the
// script through the script-network bridge, so the page's fetch policy and
// the shield decide whether it loads, then hands the source to the host,
// which runs it on a thread of its own (worker_host.rs, worker_prelude.js).
//
// Messages are structured clones (structured_clone.js), as strings between the threads. The worker's replies are drained by
// `__rk.workersPump` (the host registry, #1123), which the timer pump calls, so every handler
// runs on the page's own thread and turn.
(function () {
    var g = globalThis;
    // The structured clone helper (structured_clone.js, evaluated just before this): held here, not left
    // on the page's global, where page script could enumerate or call it.
    var sc = g.__rk_sc;
    delete g.__rk_sc;
    if (typeof g.__rustkit_worker_spawn !== 'function') return;
    var spawn = g.__rustkit_worker_spawn, post = g.__rustkit_worker_post,
        poll = g.__rustkit_worker_poll, reply = g.__rustkit_worker_reply, terminate = g.__rustkit_worker_terminate;
    // Captured once; page script does not get the bridge.
    ['spawn', 'post', 'poll', 'reply', 'terminate'].forEach(function (n) { delete g['__rustkit_worker_' + n]; });
    var states = new WeakMap();
    // The built-ins this script uses, held from install as the event code holds its own: later page
    // script that replaces them does not reach the worker records.
    var uncurry = Function.prototype.bind.bind(Function.prototype.call), objCreate = Object.create;
    var stateOf = uncurry(WeakMap.prototype.get), setState = uncurry(WeakMap.prototype.set);
    var objKeys = Object.keys, isArray = Array.isArray, jsonParse = JSON.parse, jsonStringify = JSON.stringify;
    var indexOf = uncurry(Array.prototype.indexOf);
    // The JavaScript MIME types (MIME Sniffing 4.6): importScripts refuses any other (NetworkError), as
    // Chrome does. A worker's own script is not held to it (Chrome runs one served as text/plain).
    var JS_MIME = ['application/ecmascript', 'application/javascript', 'application/x-ecmascript',
        'application/x-javascript', 'text/ecmascript', 'text/javascript', 'text/javascript1.0', 'text/javascript1.1',
        'text/javascript1.2', 'text/javascript1.3', 'text/javascript1.4', 'text/javascript1.5', 'text/jscript',
        'text/livescript', 'text/x-ecmascript', 'text/x-javascript'];
    function isJavaScript(headers) {
        for (var i = 0; headers && i < headers.length; i++) {
            if (String(headers[i][0]).toLowerCase() !== 'content-type') continue;
            var essence = String(headers[i][1]).split(';')[0].trim().toLowerCase();
            return indexOf(JS_MIME, essence) >= 0;
        }
        return false;
    }
    // A worker script of an image, audio, video or text/csv type is refused (Fetch, "should response to
    // request be blocked due to its MIME type"), as in Chrome; any other type runs.
    function blockedScriptType(headers) {
        for (var i = 0; headers && i < headers.length; i++) {
            if (String(headers[i][0]).toLowerCase() !== 'content-type') continue;
            var essence = String(headers[i][1]).split(';')[0].trim().toLowerCase();
            return /^(audio|image|video)\//.test(essence) || essence === 'text/csv';
        }
        return false;
    }
    // A request descriptor with every key the bridge reads, so it reads none through a prototype.
    function scriptRequest(url, destination, mode) {
        var d = objCreate(null);
        d.method = 'GET'; d.url = url; d.headers = []; d.body_b64 = null; d.mode = mode;
        d.credentials = 'same-origin'; d.redirect = 'follow'; d.destination = destination;
        return d;
    }
    var live = {};
    var MAX_QUEUED = 1024;

    function fromBase64(b64) {
        var s = g.atob(b64 || ''), out = new Uint8Array(s.length);
        for (var i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
        return out;
    }
    function dom(message, name) {
        return typeof g.DOMException === 'function' ? new g.DOMException(message, name) : new Error(message);
    }
    function fire(w, ev) {
        try { w.dispatchEvent(ev); } catch (e) { try { __rk.reportError(e); } catch (_) {} }
    }
    function failLoad(w, st) {
        // Chrome fires an `error` event on the Worker, asynchronously.
        g.setTimeout(function () {
            if (st.dead) return;
            st.dead = true;
            // A plain Event, not an ErrorEvent, as in Chrome.
            fire(w, new g.Event('error'));
        }, 0);
    }
    // The text of a data: URL, or null when it is malformed.
    function dataText(href) {
        var comma = href.indexOf(',');
        if (comma < 0) return null;
        var meta = href.slice(5, comma), body = href.slice(comma + 1);
        try {
            if (/;base64$/i.test(meta)) return new g.TextDecoder().decode(fromBase64(g.decodeURIComponent(body)));
            return g.decodeURIComponent(body);
        } catch (_) { return null; }
    }
    function encode(message, transfer) {
        return sc.serialize(message, transfer, 'Worker');
    }

    // importScripts: fetch the scripts under the page's fetch policy and
    // answer the worker, which waits for the whole set.
    function serviceFetch(st, rid, urlsJson) {
        var urls = null, id = st.id;
        try { urls = jsonParse(urlsJson); } catch (_) {}
        // A malformed request is answered with nothing loaded (importScripts then fails with its
        // NetworkError); the worker waits for an answer, so it always gets one.
        if (!isArray(urls) || !urls.length) {
            if (!st.dead) reply(id, rid, '[]');
            return;
        }
        var out = [], left = urls.length, net = __rk.net;
        function settle(i, value) {
            out[i] = value;
            if (--left === 0 && !st.dead && st.id === id) reply(id, rid, jsonStringify(out));
        }
        urls.forEach(function (raw, i) {
            var href;
            try { href = new g.URL(raw, st.href).href; } catch (_) { settle(i, { ok: false }); return; }
            // Cross-origin imports need CORS here (Chrome's classic imports are no-cors): stricter, not looser.
            var nid = net ? net.request(scriptRequest(href, 'script', 'cors'), function (r) {
                if (!r.ok || r.status < 200 || r.status >= 300 || !isJavaScript(r.headers)) { settle(i, { ok: false }); return; }
                try { settle(i, { ok: true, src: new g.TextDecoder().decode(fromBase64(r.body_b64)) }); }
                catch (_) { settle(i, { ok: false }); }
            }) : 0;
            if (!nid) settle(i, { ok: false });
        });
    }

    // One turn's budget of worker items, shared by all the page's workers (the host caps a poll at the same
    // number). Each turn starts after the worker the last one ended on, so a busy worker cannot starve the rest.
    var TURN_ITEMS = 256, nextWorker = 0;
    function pump() {
        var ran = 0, left = TURN_ITEMS, keys = objKeys(live), n = keys.length;
        var start = n ? nextWorker % n : 0;
        for (var j = 0; j < n && left > 0; j++) {
            var key = keys[(start + j) % n];
            nextWorker = start + j + 1;
            var st = live[key];
            if (!st || !st.id) continue;
            var raw = poll(st.id, left);
            if (raw === 'null') { delete live[key]; continue; }
            // One malformed item costs only itself: the rest of this worker's items and the other
            // workers' still run this turn.
            var items = null;
            try { items = jsonParse(raw); } catch (_) {}
            if (!isArray(items)) continue;
            left -= items.length;
            for (var i = 0; i < items.length; i++) {
                if (st.dead) break;
                var it = items[i];
                ran++;
                if (it.k === 'm') {
                    var data;
                    try { data = sc.deserialize(it.d); } catch (_) { fire(st.w, new g.MessageEvent('messageerror', { origin: '' })); continue; }
                    fire(st.w, new g.MessageEvent('message', { data: data, origin: '' }));
                } else if (it.k === 'e') {
                    var info = null, ev;
                    try { info = jsonParse(it.m); } catch (_) {}
                    if (info === null || typeof info !== 'object') info = {};
                    if (typeof g.ErrorEvent === 'function') {
                        ev = new g.ErrorEvent('error', {
                            message: info.m, filename: info.f || st.href, lineno: info.l, colno: info.c, cancelable: true
                        });
                    } else {
                        ev = new g.Event('error', { cancelable: true });
                    }
                    fire(st.w, ev);
                } else if (it.k === 'f') {
                    serviceFetch(st, it.r, it.u);
                } else if (it.k === 'x') {
                    delete live[key];
                }
            }
        }
        return ran;
    }

    function handlerProperty(type) {
        return {
            configurable: true,
            enumerable: true,
            get: function () { var st = stateOf(states, this); return st ? st.on[type] || null : null; },
            set: function (fn) {
                var st = stateOf(states, this);
                if (!st) return;
                st.on[type] = typeof fn === 'function' ? fn : null;
                if (!st.reg[type]) {
                    st.reg[type] = true;
                    var w = this;
                    this.addEventListener(type, function (ev) { if (st.on[type]) st.on[type].call(w, ev); });
                }
            }
        };
    }

    class Worker extends g.EventTarget {
        constructor(url, options) {
            if (arguments.length < 1) {
                throw new TypeError("Failed to construct 'Worker': 1 argument required, but only 0 present.");
            }
            super();
            var u;
            try {
                u = new g.URL(String(url), g.document && g.document.baseURI ? g.document.baseURI : undefined);
            } catch (e) {
                throw dom("Failed to construct 'Worker': '" + String(url) + "' is not a valid URL.", 'SyntaxError');
            }
            // A script from another origin (another scheme, host or port, CORS headers or not) and a
            // `javascript:` URL are refused at the call, before any request, as Chrome does: pages catch the
            // SecurityError to fall back to a blob: worker. data: and blob: URLs take their own paths below.
            // A page whose origin is opaque ('null': a data: URL page, a sandboxed frame, a document with no
            // URL) matches no origin, so it may start only data: and blob: workers, as in Chrome.
            var pageOrigin = g.location ? String(g.location.origin) : 'null';
            if (u.protocol !== 'data:' && u.protocol !== 'blob:'
                && (u.protocol === 'javascript:' || pageOrigin === 'null' || u.origin !== pageOrigin)) {
                throw dom("Failed to construct 'Worker': Script at '" + u.href + "' cannot be accessed from origin '" + pageOrigin + "'.", 'SecurityError');
            }
            var st = { w: this, href: u.href, id: 0, dead: false, queue: [], net: 0, on: {}, reg: {} };
            setState(states, this, st);
            var name = options && options.name !== undefined ? String(options.name) : '';
            var self = this;
            // The source is in hand (blob:, data:) or comes over the bridge.
            function begin(source) {
                if (st.dead) return;
                var loc = {
                    href: u.href, origin: u.origin, protocol: u.protocol, host: u.host, hostname: u.hostname,
                    port: u.port, pathname: u.pathname, search: u.search, hash: u.hash,
                    ua: g.navigator ? String(g.navigator.userAgent) : ''
                };
                var id = spawn(source, name, jsonStringify(loc));
                if (!id) { failLoad(self, st); return; }
                st.id = id;
                live[id] = st;
                for (var i = 0; i < st.queue.length; i++) post(id, st.queue[i]);
                st.queue = [];
            }
            if (u.protocol === 'blob:') {
                // Resolved now, as Chrome does: revoking right after `new Worker` does not matter.
                // The bytes are the host's blob store's (createObjectURL put them there).
                // Asked as this document's realm, as its XMLHttpRequest asks (web_xhr.js).
                var found = __rk.blobGet(__realm, u.href.split('#')[0]);
                if (!found) { failLoad(this, st); return; }
                var blobText;
                try { blobText = new g.TextDecoder().decode(new Uint8Array(found[0])); } catch (_) { failLoad(this, st); return; }
                g.queueMicrotask(function () { begin(blobText); });
                return;
            }
            if (u.protocol === 'data:') {
                var text = dataText(u.href);
                if (text === null) { failLoad(this, st); return; }
                g.queueMicrotask(function () { begin(text); });
                return;
            }
            var net = __rk.net;
            if (!net) { failLoad(this, st); return; }
            // Same-origin, as the spec's worker script request: a redirect off the origin fails.
            st.net = net.request(scriptRequest(u.href, 'worker', 'same-origin'), function (r) {
                st.net = 0;
                if (st.dead) return;
                if (!r.ok || r.status < 200 || r.status >= 300 || blockedScriptType(r.headers)) { failLoad(self, st); return; }
                var source;
                try { source = new g.TextDecoder().decode(fromBase64(r.body_b64)); } catch (_) { failLoad(self, st); return; }
                begin(source);
            });
            if (!st.net) failLoad(this, st);
        }
        postMessage(message, transfer) {
            if (arguments.length < 1) {
                throw new TypeError("Failed to execute 'postMessage' on 'Worker': 1 argument required, but only 0 present.");
            }
            var st = stateOf(states, this);
            if (!st) throw new TypeError('Illegal invocation');
            var json = encode(message, transfer);
            if (st.dead) return;
            if (st.id) post(st.id, json);
            else if (st.queue.length < MAX_QUEUED) st.queue.push(json);
        }
        terminate() {
            var st = stateOf(states, this);
            if (!st) throw new TypeError('Illegal invocation');
            if (st.dead) return;
            st.dead = true;
            st.queue = [];
            if (st.net) { try { __rk.net.cancel(st.net); } catch (_) {} st.net = 0; }
            if (st.id) { var id = st.id; delete live[id]; st.id = 0; terminate(id); }
        }
    }
    ['message', 'messageerror', 'error'].forEach(function (type) {
        Object.defineProperty(Worker.prototype, 'on' + type, handlerProperty(type));
    });
    Object.defineProperty(Worker.prototype, Symbol.toStringTag, { value: 'Worker', configurable: true });
    Object.defineProperty(g, 'Worker', { value: Worker, writable: true, configurable: true, enumerable: false });
    // Called by the timer pump (bindings lib.rs) through the host registry; nothing on the page's global.
    __rk.workersPump = pump;
})();
