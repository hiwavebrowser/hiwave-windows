// The script-network bridge: the ONLY way page script asks for the network.
//
// This file gives script no network access. It is a queue and a table of
// callbacks. XMLHttpRequest and fetch (built on it) put a request descriptor
// on the queue and wait; the engine takes the queue, runs each request under
// its FetchPolicy (the one allow/deny layer, in rustkit-net), and delivers
// the result back. A bindings instance the engine never enabled the bridge
// on has no __rk.net, so nothing built on it can exist there: never a
// permissive stand-in.
//
// Installed by DomBindings::enable_net_bridge, not at startup.
(function () {
    if (__rk.net) return;

    // Bounds what a page can park before the engine drains: past it a
    // request is refused (0) and the surface reports a network error. The
    // per-page request budgets live in the policy; this bounds memory.
    // 1024: a page may start a few hundred requests in one turn (JetStream
    // 2.2's preload starts 315); at 64 the rest failed at once.
    var MAX_QUEUED = 1024;
    // ... and in bytes: request bodies sit in the queue as base64 until the
    // engine takes it, and `take` serialises all of it in one call. 64 MiB
    // of encoded body parked at once; a request past it is refused like one
    // past the count.
    var MAX_QUEUED_BODY = 64 * 1024 * 1024;
    var queuedBody = 0;

    // The engine's side of this file (`take`, `shelve`, `unshelve`,
    // `waiting`) runs where script is not interrupted, so it calls nothing
    // a page can replace: the built-ins are taken now, before any page
    // script, and the tables and what is queued inherit from nothing.
    var create = Object.create, keys = Object.keys, stringify = JSON.stringify, parse = JSON.parse,
        setProto = Object.setPrototypeOf, defineOwn = Object.defineProperty, U8 = Uint8Array,
        text = String, number = Number, TypeErr = TypeError,
        takeBytes = __rk.takeBytes, base64Encode = __rk.base64Encode;
    // Called on the host object: the realm's `error` event comes first.
    function reportError(e) { __rk.reportError(e); }
    function bare(o) { return setProto(o, null); }
    // The queue is a table of the engine's own: slots 0..queued-1 in the
    // order the requests were made, written and read by index here and
    // passed to no method. What it holds is plain data (strings made
    // with the `String` of install time, and tables of them).
    var queue = create(null), queued = 0;
    function emptyQueue() { queue = create(null); queued = 0; queuedBody = 0; }
    var pending = create(null);
    // Callbacks set aside because the JavaScript budget was spent before
    // their answer could be delivered. fail_all does not settle them; the
    // engine puts them back when a later turn can run them.
    var shelved = create(null);
    var nextId = 1;

    function pairs(source) {
        var out = bare([]);
        for (var i = 0; source && i < source.length; i++) {
            out[i] = bare([text(source[i][0]), text(source[i][1])]);
        }
        return out;
    }

    function settle(id, result) {
        var done = pending[id];
        if (!done) return false;
        delete pending[id];
        try {
            done(result);
        } catch (e) {
            try { reportError(e); } catch (_) {}
        }
        return true;
    }

    var bridge = bare({
        // desc: { method, url, headers: [[name, value]], body_b64, mode,
        // credentials, redirect, destination }. `done` is called once with the
        // delivered result. Returns the request id, or 0 when refused.
        request: function (desc, done) {
            if (typeof done !== 'function') throw new TypeErr('a request needs a callback');
            if (queued >= MAX_QUEUED) return 0;
            var body = desc.body_b64 == null ? null : text(desc.body_b64);
            if (body !== null && queuedBody + body.length > MAX_QUEUED_BODY) return 0;
            if (body !== null) queuedBody += body.length;
            var id = nextId++;
            var item = bare({
                id: id,
                method: text(desc.method || 'GET'),
                url: text(desc.url),
                headers: pairs(desc.headers),
                body_b64: body,
                mode: text(desc.mode || 'cors'),
                credentials: text(desc.credentials || 'same-origin'),
                redirect: text(desc.redirect || 'follow'),
                destination: text(desc.destination || 'fetch')
            });
            // Reading `desc` may have run page script that asked again:
            // the slot is chosen now.
            if (queued >= MAX_QUEUED) { if (body !== null) queuedBody -= body.length; return 0; }
            queue[queued++] = item;
            pending[id] = done;
            return id;
        },

        // Abort: the callback is dropped (a response that still arrives is
        // ignored) and a request not yet taken never leaves the page.
        cancel: function (id) {
            delete pending[id];
            for (var i = 0; i < queued; i++) {
                if (queue[i].id === id) {
                    if (queue[i].body_b64 !== null) queuedBody -= queue[i].body_b64.length;
                    for (queued--; i < queued; i++) queue[i] = queue[i + 1];
                    delete queue[queued];
                    break;
                }
            }
        },

        // Engine side.
        take: function () {
            var taken = bare([]);
            for (var i = 0; i < queued; i++) taken[i] = queue[i];
            emptyQueue();
            return stringify(taken);
        },
        // A response's body does not travel in `json`: the engine leaves
        // the bytes with the host and they are taken here, once, as
        // `result.bytes` (a Uint8Array). `result.body_b64` is still there
        // for whoever reads it, computed when read.
        deliver: function (json) {
            var result = bare(parse(json));
            if (result.ok && result.body_b64 === undefined && pending[result.id]) {
                var bytes = new U8(takeBytes(result.id) || 0);
                var d = create(null);
                d.value = bytes;
                defineOwn(result, 'bytes', d);
                d = create(null);
                d.get = function () { return base64Encode(bytes); };
                d.enumerable = true; d.configurable = true;
                defineOwn(result, 'body_b64', d);
            }
            return settle(result.id, result);
        },
        // Move one waiting callback out of fail_all's reach. The answer is
        // kept by the engine and delivered on a later turn.
        shelve: function (id) {
            var done = pending[id];
            if (!done) return false;
            delete pending[id];
            shelved[id] = done;
            return true;
        },
        unshelve: function () {
            var n = 0;
            for (var id in shelved) {
                pending[id] = shelved[id];
                n++;
            }
            shelved = create(null);
            return n;
        },
        // Whatever is still waiting completes with an error (the engine's
        // round or budget ran out). A failure handler that asks again (a
        // retry loop) gets the same answer, but only for a few passes: what
        // is still waiting after that is dropped, so a page cannot keep the
        // engine here by retrying forever.
        fail_all: function (reason) {
            var failed = 0;
            for (var pass = 0; pass < 3; pass++) {
                emptyQueue();
                var ids = keys(pending);
                if (ids.length === 0) break;
                for (var i = 0; i < ids.length; i++) {
                    var id = number(ids[i]);
                    if (settle(id, bare({ id: id, ok: false, error: text(reason) }))) failed++;
                }
            }
            emptyQueue();
            pending = create(null);
            return failed;
        },
        waiting: function () {
            return keys(pending).length;
        },
        queued: function () {
            return queued;
        }
    });

    __rk.net = bridge;
})();
