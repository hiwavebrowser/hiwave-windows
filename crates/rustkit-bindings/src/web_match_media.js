// window.matchMedia and MediaQueryList (CSSOM View §4.2) over the engine's
// media query evaluator, the one that decides `@media` rules in sheets
// (rustkit-css media.rs, through web_match_media.rs). The stub it replaces
// answered `matches: false` for every query, so a script that picks its
// layout from a media query always took the narrowest branch (netflix.com's
// hero form stayed stacked where Chrome lays it out in one row).
//
// `matches` is read against the viewport as it stands (`window.innerWidth`
// / `innerHeight`), so it is right after a resize even before the page
// hears about it. `change` fires from `__rustkit_media_changes()`, which
// the engine runs after the `resize` event (the "evaluate media queries and
// report changes" step of update the rendering, HTML §8.1.7.3), for every
// list whose answer flipped since it was last reported. A list joins the
// document's list of media query lists when it gets a `change` listener
// or an `onchange` handler; the comparison starts at the answer it had
// then. The report drops a list that no longer has either (its listeners
// removed or `once`, `onchange` set back to null): nothing could hear it,
// and Chrome collects such lists too (CSSOM View §4.2 keeps them only
// while reachable), so a page that churns listeners (React's
// useMediaQuery on a long-lived SPA) does not grow the list for the life
// of the document. A list that listens again joins afresh, comparing
// from then.
//
// `media` is the serialized query: lowercased, whitespace normalized, an
// invalid query `not all` (and never a throw).
(function (g) {
    var EventTarget = g.EventTarget, MediaQueryList = g.MediaQueryList;
    if (typeof EventTarget !== 'function' || typeof MediaQueryList !== 'function') return;
    var serialize = g.__rustkit_media_serialize, evaluate = g.__rustkit_media_matches;
    if (typeof serialize !== 'function' || typeof evaluate !== 'function') return;
    try { delete g.__rustkit_media_serialize; delete g.__rustkit_media_matches; } catch (_) {}

    var proto = MediaQueryList.prototype;
    // The DOM install's listener registry (`hasListener(target, type)`).
    var HOOKS = g.__rkDomHooks || {};
    // MediaQueryList -> { media, w, h, matches, reported, onchange }
    var STATE = new WeakMap();
    // The lists that can hear `change`, in the order they started to.
    var live = [];

    function method(name, fn) {
        Object.defineProperty(proto, name, { value: fn, writable: true, configurable: true, enumerable: false });
    }
    function accessor(name, get, set) {
        Object.defineProperty(proto, name, { get: get, set: set, configurable: true, enumerable: true });
    }
    function size(v) {
        v = Number(v);
        return v > 0 && v < Infinity ? v : 0;
    }
    function state(mql) {
        var s = STATE.get(mql);
        if (!s) throw new TypeError('Illegal invocation');
        return s;
    }
    // The answer for the viewport as it stands; re-evaluated only when the
    // viewport moved since the last read.
    function current(s) {
        var w = size(g.innerWidth), h = size(g.innerHeight);
        if (s.w !== w || s.h !== h) {
            s.w = w;
            s.h = h;
            s.matches = !!evaluate(s.media, w, h);
        }
        return s.matches;
    }
    function track(mql, s) {
        if (live.indexOf(mql) >= 0) return;
        s.reported = current(s);
        live.push(mql);
    }

    accessor('media', function () { return state(this).media; });
    accessor('matches', function () { return current(state(this)); });
    accessor('onchange',
        function () { return state(this).onchange; },
        function (v) {
            var s = state(this);
            s.onchange = typeof v === 'function' ? v : null;
            if (s.onchange) track(this, s);
        });
    method('addEventListener', function (type, cb, opts) {
        var s = state(this);
        EventTarget.prototype.addEventListener.call(this, type, cb, opts);
        if (String(type) === 'change') track(this, s);
    });
    method('addListener', function (cb) {
        if (cb != null) this.addEventListener('change', cb);
    });
    method('removeListener', function (cb) {
        if (cb != null) this.removeEventListener('change', cb);
    });

    function matchMedia(query) {
        if (arguments.length < 1) {
            throw new TypeError("Failed to execute 'matchMedia' on 'Window': 1 argument required, but only 0 present.");
        }
        var media = String(serialize(String(query)));
        var w = size(g.innerWidth), h = size(g.innerHeight);
        var matches = !!evaluate(media, w, h);
        var mql = Object.create(proto);
        STATE.set(mql, { media: media, w: w, h: h, matches: matches, reported: matches, onchange: null });
        return mql;
    }
    Object.defineProperty(g, 'matchMedia', { value: matchMedia, writable: true, configurable: true, enumerable: true });

    function changeEvent(s, matches) {
        var init = { media: s.media, matches: matches };
        if (typeof g.MediaQueryListEvent === 'function') {
            try { return new g.MediaQueryListEvent('change', init); } catch (_) {}
        }
        var e = new g.Event('change');
        e.media = s.media;
        e.matches = matches;
        return e;
    }
    // Can anything hear `change` from this list right now?
    function heard(mql, s) {
        if (s.onchange !== null) return true;
        return typeof HOOKS.hasListener === 'function' ? HOOKS.hasListener(mql, 'change') : true;
    }
    // Report the lists whose answer flipped since it was last reported;
    // answers how many heard `change`. A list nothing can hear any more is
    // dropped instead (see the header). Listener exceptions go to the
    // engine's error log through dispatchEvent, as any listener's do.
    g.__rustkit_media_changes = function () {
        var lists = live.slice(), fired = 0;
        for (var i = 0; i < lists.length; i++) {
            var mql = lists[i], s = STATE.get(mql);
            if (!s) continue;
            if (!heard(mql, s)) {
                var at = live.indexOf(mql);
                if (at >= 0) live.splice(at, 1);
                continue;
            }
            var now = current(s);
            if (now === s.reported) continue;
            s.reported = now;
            mql.dispatchEvent(changeEvent(s, now));
            fired++;
        }
        return fired;
    };
})(globalThis);
