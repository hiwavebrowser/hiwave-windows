// Session history of one document (HTML §7.4) and the anchor URL parts
// (HTMLHyperlinkElementUtils, HTML §4.6.3).
//
// There is no real navigation here: pushState/replaceState add or rewrite
// entries of the current document, and back/forward/go move between them,
// updating `location` and firing `popstate` from a task. Entries made by an
// engine navigation (`set_location`) start a fresh list.
(function (g) {
    var loc = g.location;
    var entries, index;

    function domError(message, name) {
        return new DOMException(message, name);
    }
    // Script asks for another document (HTML §7.4.2 "navigate"). The page
    // is not replaced here: the request goes to the engine, and whoever
    // embeds it (the live app) loads the URL. A capture never takes it.
    var nav = g.__rkNavInternals || {};
    delete g.__rkNavInternals;
    var hrefValue = loc.href, syncing = false;
    function sameDocument(a, b) {
        return a.href.split('#')[0] === b.href.split('#')[0];
    }
    function requestNavigation(target) {
        if (!nav.navigate || (target.protocol !== 'http:' && target.protocol !== 'https:')) return;
        nav.navigate(target.href);
    }
    // `location.href = url`, and through it assign(), replace() and
    // `document.location = url`. `location` keeps the document's URL until
    // a navigation replaces the document; it used to read back the string
    // script assigned, relative or not. Another fragment of this document
    // is the fragment navigation; anything else is a request.
    function go(target) {
        var current;
        try { current = new URL(hrefValue); } catch (e) { return; }
        if (!sameDocument(current, target)) return requestNavigation(target);
        if (target.href !== current.href && target.hash !== '') g.__rustkit_fragment_navigation(target.href);
    }
    Object.defineProperty(loc, 'href', {
        get: function () { return hrefValue; },
        set: function (v) {
            if (syncing) { hrefValue = String(v); return; }
            var target;
            try { target = new URL(String(v), hrefValue); } catch (e) { return; }
            go(target);
        },
        enumerable: true, configurable: true
    });
    function setLocation(href) {
        var u = new URL(href);
        syncing = true;
        try {
            ['href', 'protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin']
                .forEach(function (k) { loc[k] = u[k]; });
        } finally { syncing = false; }
        if (g.document) g.document.URL = u.href;
    }
    // `window.location = url` is the same request. Until this was an
    // accessor the assignment replaced the Location object with a string.
    try {
        Object.defineProperty(g, 'location', {
            get: function () { return loc; },
            set: function (v) { loc.href = String(v); },
            enumerable: true, configurable: true
        });
    } catch (e) {}
    // The engine's own write of a new document's URL.
    g.__rustkit_location_sync = function (href) {
        syncing = true;
        try { loc.href = href; } finally { syncing = false; }
    };
    // A new document: one entry, the current URL, no state.
    g.__rustkit_history_reset = function () {
        entries = [{ url: loc.href, state: null }];
        index = 0;
    };
    g.__rustkit_history_reset();

    // "Can have its URL rewritten": same origin, and for a non-http(s) URL
    // only the fragment may change.
    function rewritable(doc, target) {
        if (doc.protocol !== target.protocol || doc.username !== target.username ||
            doc.password !== target.password || doc.host !== target.host) return false;
        if (doc.protocol === 'http:' || doc.protocol === 'https:') return true;
        return doc.pathname === target.pathname && doc.search === target.search;
    }
    function update(method, args, push) {
        if (args.length < 2) {
            throw new TypeError("Failed to execute '" + method + "' on 'History': 2 arguments required, but only " + args.length + ' present.');
        }
        var state = structuredClone(args[0]);
        var url = loc.href, raw = args[2];
        if (raw !== undefined && raw !== null) {
            var target;
            try { target = new URL(String(raw), loc.href); } catch (e) { target = null; }
            if (!target || !rewritable(new URL(loc.href), target)) {
                throw domError("Failed to execute '" + method + "' on 'History': A history state object with URL '" +
                    String(raw) + "' cannot be created in a document with origin '" + loc.origin + "'.", 'SecurityError');
            }
            url = target.href;
        }
        var entry = { url: url, state: state };
        if (push) {
            entries.length = index + 1;
            entries.push(entry);
            index++;
        } else {
            entries[index] = entry;
        }
        setLocation(url);
    }
    // Traversal is queued, so the delta is applied when the task runs.
    function traverse(delta) {
        delta = Number(delta) || 0;
        if (delta === 0) return;
        setTimeout(function () {
            var to = index + delta;
            if (to < 0 || to >= entries.length) return;
            var oldURL = loc.href;
            index = to;
            setLocation(entries[to].url);
            g.dispatchEvent(new PopStateEvent('popstate', { state: entries[to].state }));
            if (new URL(oldURL).hash !== loc.hash) {
                g.dispatchEvent(new HashChangeEvent('hashchange', { oldURL: oldURL, newURL: loc.href }));
            }
        }, 0);
    }

    // The engine followed a link to a fragment of this document (HTML
    // §7.4.2.3.3): a new entry, the new URL, then `hashchange`.
    g.__rustkit_fragment_navigation = function (href) {
        var oldURL = loc.href;
        if (oldURL === href) return;
        entries.length = index + 1;
        entries.push({ url: href, state: null });
        index++;
        setLocation(href);
        g.dispatchEvent(new HashChangeEvent('hashchange', { oldURL: oldURL, newURL: loc.href }));
    };

    // What script's `link.click()` does once no listener cancelled it: the
    // nearest link up from the target is followed. A fragment of this
    // document is the same navigation the engine makes for the user's
    // click; anything that opens elsewhere (target, download) is left. A
    // form control between the target and the link has the click's
    // activation itself, so the link is not followed.
    var OWN_ACTIVATION = /^(button|input|select|textarea)$/;
    g.__rustkit_follow_link = function (el) {
        var a = el;
        while (a && !(a.nodeType === 1 && (a.localName === 'a' || a.localName === 'area') && a.hasAttribute('href'))) {
            if (a.nodeType === 1 && OWN_ACTIVATION.test(a.localName)) return;
            a = a.parentNode;
        }
        if (!a || a.hasAttribute('download')) return;
        var where = (a.getAttribute('target') || '').toLowerCase();
        if (where !== '' && where !== '_self') return;
        var target;
        try { target = new URL(a.getAttribute('href'), hrefValue); } catch (e) { return; }
        go(target);
    };

    function History() { throw new TypeError('Illegal constructor'); }
    var hp = History.prototype;
    Object.defineProperty(hp, 'length', { get: function () { return entries.length; }, enumerable: true, configurable: true });
    Object.defineProperty(hp, 'state', { get: function () { return entries[index].state; }, enumerable: true, configurable: true });
    Object.defineProperty(hp, 'scrollRestoration', { value: 'auto', writable: true, enumerable: true, configurable: true });
    hp.pushState = function (state, title, url) { update('pushState', arguments, true); };
    hp.replaceState = function (state, title, url) { update('replaceState', arguments, false); };
    hp.back = function () { traverse(-1); };
    hp.forward = function () { traverse(1); };
    hp.go = function (delta) { traverse(delta); };
    Object.defineProperty(hp, Symbol.toStringTag, { value: 'History', configurable: true });
    Object.defineProperty(g, 'History', { value: History, writable: true, configurable: true });
    g.history = Object.create(hp);

    // ---- HTMLAnchorElement / HTMLAreaElement URL parts.
    // The href attribute parsed against the document URL, or null.
    function parsed(el) {
        var href = el.getAttribute('href');
        if (href === null) return null;
        try { return new URL(href, loc.href); } catch (e) { return null; }
    }
    var PARTS = ['protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash'];
    [g.HTMLAnchorElement, g.HTMLAreaElement].forEach(function (C) {
        if (typeof C !== 'function') return;
        var p = C.prototype;
        Object.defineProperty(p, 'href', {
            get: function () {
                var u = parsed(this);
                if (u) return u.href;
                var raw = this.getAttribute('href');
                return raw === null ? '' : raw;
            },
            set: function (v) { this.setAttribute('href', String(v)); },
            enumerable: true, configurable: true
        });
        Object.defineProperty(p, 'origin', {
            get: function () { var u = parsed(this); return u ? u.origin : ''; },
            enumerable: true, configurable: true
        });
        PARTS.forEach(function (k) {
            Object.defineProperty(p, k, {
                get: function () {
                    var u = parsed(this);
                    return u ? u[k] : (k === 'protocol' ? ':' : '');
                },
                // A part set on a missing or unparsable href is ignored.
                set: function (v) {
                    var u = parsed(this);
                    if (!u) return;
                    u[k] = v;
                    this.setAttribute('href', u.href);
                },
                enumerable: true, configurable: true
            });
        });
        p.toString = function () { return this.href; };
    });
})(globalThis);
