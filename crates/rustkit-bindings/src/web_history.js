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
    // A navigation that replaces the session history entry instead of
    // adding one (HTML §7.4.2.2 "navigate", historyHandling "replace"):
    // `location.replace()`, a reload, the document's own URL again, and a
    // declarative refresh. Chrome's Back does not return to them.
    var replacing = false;
    function requestNavigation(target, replace) {
        if (!nav.navigate || (target.protocol !== 'http:' && target.protocol !== 'https:')) return;
        nav.navigate(target.href, !!(replace || replacing || target.href === hrefValue));
    }
    // `location.href = url`, and through it assign(), replace() and
    // `document.location = url`. `location` keeps the document's URL until
    // a navigation replaces the document; it used to read back the string
    // script assigned, relative or not. Another fragment of this document
    // is the fragment navigation; anything else is a request.
    function go(target) {
        var current;
        try { current = new URL(hrefValue); } catch (e) { return; }
        // Only a URL with a fragment is a fragment navigation; the
        // document's own URL without one loads it again (HTML §7.4.2.2
        // step 12), as `location.href = location.href` does in Chrome.
        if (!sameDocument(current, target) || target.href.indexOf('#') < 0) return requestNavigation(target);
        if (target.href !== current.href) g.__rustkit_fragment_navigation(target.href);
    }
    // `location.reload()`: the same URL again, as a request like the rest.
    loc.reload = function () {
        try { requestNavigation(new URL(hrefValue), true); } catch (e) {}
    };
    loc.replace = function (url) {
        replacing = true;
        try { loc.href = String(url); } finally { replacing = false; }
    };
    // Location's stringifier (HTML §7.2.4): `String(location)` and
    // `new URL(path, location)` read its URL. Without it the object read as
    // `[object Location]` and squarespace's app threw Invalid URL.
    Object.defineProperty(loc, 'toString', {
        value: function toString() { return loc.href; },
        writable: false, enumerable: true, configurable: false
    });
    // `<meta http-equiv=refresh>` (HTML §4.2.5.3, "shared declarative
    // refresh steps"): once the document has loaded, the first one that
    // parses asks for its URL (or this document again) after its seconds.
    // A page that waits and then retries or moves on (a 503's "try again
    // shortly") did neither until 2026-10-16.
    function parseRefresh(content) {
        var m = /^[\t\n\f\r ]*(\d*)(\.[\d.]*)?/.exec(content);
        if (m[1] === '' && !m[2]) return null;
        var rest = content.slice(m[0].length);
        if (rest !== '' && !/^[\t\n\f\r ;,]/.test(rest)) return null;
        rest = rest.replace(/^[\t\n\f\r ]*[;,]?[\t\n\f\r ]*/, '').replace(/^url[\t\n\f\r ]*=[\t\n\f\r ]*/i, '');
        var quote = rest.charAt(0);
        if (quote === '"' || quote === "'") {
            rest = rest.slice(1);
            if (rest.indexOf(quote) >= 0) rest = rest.slice(0, rest.indexOf(quote));
        }
        return { seconds: m[1] === '' ? 0 : parseInt(m[1], 10), url: rest.replace(/[\t\n\f\r ]+$/, '') };
    }
    function refreshOnLoad() {
        var metas = g.document && g.document.getElementsByTagName ? g.document.getElementsByTagName('meta') : [];
        for (var i = 0; i < metas.length; i++) {
            var equiv = metas[i].getAttribute('http-equiv');
            if (!equiv || equiv.trim().toLowerCase() !== 'refresh') continue;
            // Scripting is on, so a `<noscript>`'s content is text in a
            // browser, not a meta: its refresh (a challenge page's way to
            // the no-JavaScript fallback) is not one.
            if (metas[i].closest('noscript')) continue;
            var refresh = parseRefresh(metas[i].getAttribute('content') || '');
            if (!refresh) continue;
            var target;
            try { target = new URL(refresh.url || hrefValue, g.document.baseURI || hrefValue); } catch (e) { return; }
            setTimeout(function () { requestNavigation(target, true); }, Math.min(refresh.seconds, 2147483) * 1000);
            return;
        }
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
    // The hash setter (HTML §7.2.4) is the fragment navigation. Script could
    // assign `location.hash` and nothing happened until 2026-10-31.
    var hashValue = loc.hash || '';
    Object.defineProperty(loc, 'hash', {
        get: function () { return hashValue; },
        set: function (v) {
            if (syncing) { hashValue = String(v); return; }
            var target;
            try { target = new URL(hrefValue.split('#')[0] + '#' + String(v).replace(/^#/, '')); } catch (e) { return; }
            // The fragment already there (none reads as empty) is no
            // navigation: `location.hash = ''` on a URL without one stays put.
            var at = hrefValue.indexOf('#');
            if (target.hash.slice(1) === (at < 0 ? '' : hrefValue.slice(at + 1))) return;
            go(target);
        },
        enumerable: true, configurable: true
    });
    g.__rustkit_location_hash_sync = function (hash) { hashValue = hash; };
    // Location's stringifier is its href (HTML §7.3). `new URL(path, location)`
    // and `String(location)` use it; without it the base is "[object Object]"
    // and the URL constructor throws Invalid URL.
    function locationString() { return hrefValue; }
    loc.toString = locationString;
    loc.valueOf = locationString;
    if (typeof Symbol === 'function') loc[Symbol.toPrimitive] = locationString;
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
    var listening = false;
    g.__rustkit_history_reset = function () {
        entries = [{ url: loc.href, state: null }];
        index = 0;
        // Listened for once the document is bound: window's listeners
        // are not all in place when this file runs.
        if (!listening && g.document && g.addEventListener) {
            listening = true;
            g.addEventListener('load', refreshOnLoad);
        }
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
    // The same URL again adds no entry and hears no `hashchange`, but scrolls.
    g.__rustkit_fragment_navigation = function (href) {
        var oldURL = loc.href;
        if (oldURL !== href) {
            entries.length = index + 1;
            entries.push({ url: href, state: null });
            index++;
            setLocation(href);
        }
        g.__rustkit_scroll_to_location_fragment();
        if (oldURL !== href) {
            g.dispatchEvent(new HashChangeEvent('hashchange', { oldURL: oldURL, newURL: loc.href }));
        }
    };
    // Scroll to what the URL's fragment names, if it has one (a URL ending in
    // a bare `#` has an empty one, whose `hash` reads as none).
    g.__rustkit_scroll_to_location_fragment = function () {
        var at = loc.href.indexOf('#');
        if (at >= 0 && typeof g.__rkScrollToFragment === 'function') g.__rkScrollToFragment(loc.href.slice(at + 1));
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
