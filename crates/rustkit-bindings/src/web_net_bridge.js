// The script-network bridge: the ONLY way page script asks for the network.
//
// This file gives script no network access. It is a queue and a table of
// callbacks. XMLHttpRequest and fetch (built on it) put a request descriptor
// on the queue and wait; the engine takes the queue, runs each request under
// its FetchPolicy (the one allow/deny layer, in rustkit-net), and delivers
// the result back. A bindings instance the engine never enabled the bridge
// on has no __rustkit_net, so nothing built on it can exist there: never a
// permissive stand-in.
//
// Installed by DomBindings::enable_net_bridge, not at startup.
(function () {
    if (window.__rustkit_net) return;

    // Bounds what a page can park before the engine drains: past it a
    // request is refused (0) and the surface reports a network error. The
    // per-page request budgets live in the policy; this bounds memory.
    var MAX_QUEUED = 64;

    var queue = [];
    var pending = {};
    var nextId = 1;

    function pairs(source) {
        var out = [];
        for (var i = 0; source && i < source.length; i++) {
            out.push([String(source[i][0]), String(source[i][1])]);
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
            window.__rustkit_errors.push(String(e));
        }
        return true;
    }

    var bridge = {
        // desc: { method, url, headers: [[name, value]], body_b64, mode,
        // credentials, redirect, destination }. `done` is called once with the
        // delivered result. Returns the request id, or 0 when refused.
        request: function (desc, done) {
            if (typeof done !== 'function') throw new TypeError('a request needs a callback');
            if (queue.length >= MAX_QUEUED) return 0;
            var id = nextId++;
            queue.push({
                id: id,
                method: String(desc.method || 'GET'),
                url: String(desc.url),
                headers: pairs(desc.headers),
                body_b64: desc.body_b64 == null ? null : String(desc.body_b64),
                mode: String(desc.mode || 'cors'),
                credentials: String(desc.credentials || 'same-origin'),
                redirect: String(desc.redirect || 'follow'),
                destination: String(desc.destination || 'fetch')
            });
            pending[id] = done;
            return id;
        },

        // Abort: the callback is dropped (a response that still arrives is
        // ignored) and a request not yet taken never leaves the page.
        cancel: function (id) {
            delete pending[id];
            for (var i = 0; i < queue.length; i++) {
                if (queue[i].id === id) {
                    queue.splice(i, 1);
                    break;
                }
            }
        },

        // Engine side.
        take: function () {
            return JSON.stringify(queue.splice(0));
        },
        deliver: function (json) {
            var result = JSON.parse(json);
            return settle(result.id, result);
        },
        // Whatever is still waiting completes with an error (the engine's
        // round or budget ran out). A failure handler that asks again (a
        // retry loop) gets the same answer, but only for a few passes: what
        // is still waiting after that is dropped, so a page cannot keep the
        // engine here by retrying forever.
        fail_all: function (reason) {
            var failed = 0;
            for (var pass = 0; pass < 3; pass++) {
                queue.splice(0);
                var ids = Object.keys(pending);
                if (ids.length === 0) break;
                for (var i = 0; i < ids.length; i++) {
                    var id = Number(ids[i]);
                    if (settle(id, { id: id, ok: false, error: String(reason) })) failed++;
                }
            }
            queue.splice(0);
            pending = {};
            return failed;
        },
        waiting: function () {
            return Object.keys(pending).length;
        },
        queued: function () {
            return queue.length;
        }
    };

    Object.defineProperty(window, '__rustkit_net', { value: bridge });
})();
