// The global scope of a dedicated worker (STR-4). It runs in a JsRuntime of
// its own, on a thread of its own, and starts from nothing: no `window`, no
// `document`, no storage, none of the page's objects. Only what a worker
// scope has is defined here. The host supplies `__rk_worker_out`,
// `__rk_worker_close`, `__rk_worker_error` and `__rk_worker_fetch`.
(function (g) {
    'use strict';
    var out = g.__rk_worker_out, closeHost = g.__rk_worker_close, errorHost = g.__rk_worker_error, fetchHost = g.__rk_worker_fetch;
    // Held here; the worker's own script does not get them.
    delete g.__rk_worker_out; delete g.__rk_worker_close; delete g.__rk_worker_error; delete g.__rk_worker_fetch;
    // The structured clone helper (structured_clone.js, evaluated just before this): held here, not left on
    // the worker's global scope.
    var sc = g.__rk_sc;
    delete g.__rk_sc;
    var listeners = Object.create(null);
    var closing = false;

    function hide(name, value) {
        Object.defineProperty(g, name, { value: value, writable: true, configurable: true, enumerable: false });
    }

    class Event {
        constructor(type) {
            this.type = String(type);
            this.target = null;
            this.currentTarget = null;
            this.defaultPrevented = false;
            this.bubbles = false;
            this.cancelable = false;
        }
        preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
        stopPropagation() {}
        stopImmediatePropagation() { this.__stop = true; }
    }
    class MessageEvent extends Event {
        constructor(type, init) {
            super(type);
            init = init || {};
            this.data = 'data' in init ? init.data : null;
            this.origin = init.origin === undefined ? '' : String(init.origin);
            this.lastEventId = '';
            this.source = null;
            this.ports = [];
        }
    }
    class ErrorEvent extends Event {
        constructor(type, init) {
            super(type);
            init = init || {};
            this.cancelable = true;
            this.message = init.message === undefined ? '' : String(init.message);
            this.filename = init.filename === undefined ? '' : String(init.filename);
            this.lineno = init.lineno | 0;
            this.colno = init.colno | 0;
            this.error = 'error' in init ? init.error : null;
        }
    }
    var CODES = { IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5,
        NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InUseAttributeError: 10,
        InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15,
        TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20, URLMismatchError: 21,
        QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25 };
    class DOMException extends Error {
        constructor(message, name) {
            super(message === undefined ? '' : String(message));
            Object.defineProperty(this, 'name', { value: name === undefined ? 'Error' : String(name), configurable: true });
        }
        get code() { return CODES[this.name] || 0; }
    }
    hide('Event', Event);
    hide('ErrorEvent', ErrorEvent);
    hide('DOMException', DOMException);
    hide('MessageEvent', MessageEvent);

    function addEventListener(type, cb) {
        if (typeof cb !== 'function' && !(cb && typeof cb.handleEvent === 'function')) return;
        var list = listeners[type] || (listeners[type] = []);
        for (var i = 0; i < list.length; i++) if (list[i] === cb) return;
        list.push(cb);
    }
    function removeEventListener(type, cb) {
        var list = listeners[type];
        if (!list) return;
        var i = list.indexOf(cb);
        if (i >= 0) list.splice(i, 1);
    }
    function dispatchEvent(ev) {
        ev.target = g;
        ev.currentTarget = g;
        var list = (listeners[ev.type] || []).slice();
        for (var i = 0; i < list.length; i++) {
            try {
                if (typeof list[i] === 'function') list[i].call(g, ev);
                else list[i].handleEvent(ev);
            } catch (e) { reportError(e, true); }
            if (ev.__stop) break;
        }
        return true;
    }
    hide('addEventListener', addEventListener);
    hide('removeEventListener', removeEventListener);
    hide('dispatchEvent', dispatchEvent);

    // onmessage and its kin are listeners registered when first assigned, so
    // their order against addEventListener calls is the order of the calls.
    ['message', 'messageerror', 'error'].forEach(function (type) {
        var handler = null, registered = false;
        Object.defineProperty(g, 'on' + type, {
            configurable: true,
            enumerable: true,
            get: function () { return handler; },
            set: function (fn) {
                handler = typeof fn === 'function' ? fn : null;
                if (!registered) {
                    registered = true;
                    addEventListener(type, function (ev) { if (handler) handler.call(g, ev); });
                }
            }
        });
    });

    // An uncaught error: an `error` event on the scope first, and if no
    // handler cancels it, an `error` event on the Worker object in the page.
    function reportError(e, inHandler) {
        var msg = 'Uncaught ' + (e && typeof e === 'object' && e.name !== undefined ? e.name + ': ' + e.message : String(e));
        var href = g.location ? g.location.href : '';
        var ev = new ErrorEvent('error', { message: msg, filename: href, lineno: 0, colno: 0, error: e });
        if (!inHandler) dispatchEvent(ev);
        if (!ev.defaultPrevented) errorHost(JSON.stringify({ m: msg, f: href, l: 0, c: 0 }));
    }

    // Timers run on the worker thread's own clock; the host sleeps until
    // `__rk_worker_next` says one is due and calls `__rk_worker_run_due`.
    var timers = [], seq = 1, nextId = 1;
    function schedule(cb, delay, args, every) {
        delay = Number(delay);
        if (!(delay > 0)) delay = 0;
        if (every && delay < 1) delay = 1;
        var id = nextId++;
        timers.push({ id: id, due: Date.now() + delay, seq: seq++, every: every ? delay : 0, cb: cb, args: args });
        return id;
    }
    function clearTimer(id) {
        for (var i = 0; i < timers.length; i++) if (timers[i].id === id) { timers.splice(i, 1); return; }
    }
    hide('setTimeout', function setTimeout(cb, delay) { return schedule(cb, delay, [].slice.call(arguments, 2), false); });
    hide('setInterval', function setInterval(cb, delay) { return schedule(cb, delay, [].slice.call(arguments, 2), true); });
    hide('clearTimeout', clearTimer);
    hide('clearInterval', clearTimer);
    hide('queueMicrotask', function queueMicrotask(cb) {
        if (typeof cb !== 'function') throw new TypeError("Failed to execute 'queueMicrotask' on 'WorkerGlobalScope': The callback provided as parameter 1 is not a function.");
        Promise.resolve().then(function () { try { cb(); } catch (e) { reportError(e); } });
    });
    hide('__rk_worker_next', function () {
        if (closing || !timers.length) return -1;
        var due = timers[0].due;
        for (var i = 1; i < timers.length; i++) if (timers[i].due < due) due = timers[i].due;
        return Math.max(0, due - Date.now());
    });
    hide('__rk_worker_run_due', function () {
        for (var guard = 0; guard < 1000 && !closing; guard++) {
            var n = Date.now(), best = -1;
            for (var i = 0; i < timers.length; i++) {
                var t = timers[i];
                if (t.due > n) continue;
                if (best < 0 || t.due < timers[best].due || (t.due === timers[best].due && t.seq < timers[best].seq)) best = i;
            }
            if (best < 0) return;
            var timer = timers[best];
            if (timer.every) {
                // Counted from its due time, so a late start does not reorder it
                // against other timers; one over a second behind is not caught up.
                timer.due += timer.every;
                if (timer.due < n - 1000) timer.due = n + timer.every;
                timer.seq = seq++;
            } else timers.splice(best, 1);
            try {
                if (typeof timer.cb === 'function') timer.cb.apply(g, timer.args);
                else (0, eval)(String(timer.cb));
            } catch (e) { reportError(e); }
        }
    });
    hide('__rk_worker_run', function (source) {
        try { (0, eval)(source); } catch (e) { reportError(e); }
    });

    hide('self', g);
    hide('name', '');
    hide('postMessage', function postMessage(message, transfer) {
        if (arguments.length < 1) throw new TypeError("Failed to execute 'postMessage' on 'DedicatedWorkerGlobalScope': 1 argument required, but only 0 present.");
        if (closing) return;
        out(sc.serialize(message, transfer, 'DedicatedWorkerGlobalScope'));
    });
    hide('close', function close() { closing = true; closeHost(); });
    // Synchronous: the worker thread waits while the page fetches the
    // scripts under its own fetch policy. Every script must load before
    // the first one runs.
    hide('importScripts', function importScripts() {
        var urls = [].map.call(arguments, String);
        if (!urls.length || closing) return;
        var raw = fetchHost(JSON.stringify(urls)), res = raw ? JSON.parse(raw) : null;
        for (var i = 0; i < urls.length; i++) {
            if (!res || !res[i] || !res[i].ok) {
                throw new DOMException("Failed to execute 'importScripts' on 'WorkerGlobalScope': The script at '" + urls[i] + "' failed to load.", 'NetworkError');
            }
        }
        for (var j = 0; j < res.length; j++) (0, eval)(res[j].src);
    });
    hide('__rk_worker_setup', function (name, locJson) {
        g.name = String(name);
        var loc = JSON.parse(locJson), ua = loc.ua || '';
        // `ua` carries the page's user agent for `navigator`; WorkerLocation has no such member.
        delete loc.ua;
        loc.toString = function () { return loc.href; };
        hide('location', loc);
        hide('navigator', { userAgent: ua, hardwareConcurrency: 1, onLine: true, language: 'en-US', languages: ['en-US'] });
    });
    hide('__rk_worker_dispatch', function (json) {
        if (closing) return;
        var data;
        try { data = sc.deserialize(json); } catch (_) { dispatchEvent(new MessageEvent('messageerror', {})); return; }
        dispatchEvent(new MessageEvent('message', { data: data }));
    });
})(globalThis);
