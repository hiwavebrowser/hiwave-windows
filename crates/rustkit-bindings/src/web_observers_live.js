// IntersectionObserver and ResizeObserver that report (Intersection Observer
// §3, Resize Observer §2), over the geometry and scroll state the engine
// publishes. They replace the inert stubs in web_observers.js, which accepted
// observe() and never called back, so content that waits on them (lazy
// images, infinite scroll, impression tracking, responsive widgets) never
// appeared.
//
// Delivery: `__rkObserversTick()` computes every observer's records from the
// geometry as it stands and calls the callbacks. It runs from a timer after
// the first observe() (the initial notification), and the engine runs it after
// a layout and after a scroll (`DomBindings::tick_observers`). Callbacks that
// observe or mutate during the tick are picked up by the next one (bounded).
//
// Stated limits: IntersectionObserver clips by the viewport (or the `root`
// element's box) and `rootMargin` only, not by the `overflow` clip of the
// target's ancestors; ResizeObserver reports the content and border box sizes
// the layout last published.
(function (g) {
    var document = g.document;
    if (!document || !g.Element) return;

    function report(e) {
        try { if (g.__rustkit_errors) g.__rustkit_errors.push(String(e)); } catch (_) {}
    }
    function requireCallback(ctor, cb) {
        if (typeof cb !== 'function') {
            throw new TypeError("Failed to construct '" + ctor + "': The callback provided as parameter 1 is not a function.");
        }
    }
    function requireElement(ctor, target) {
        if (target === null || typeof target !== 'object' || target.nodeType !== 1) {
            throw new TypeError("Failed to execute 'observe' on '" + ctor + "': parameter 1 is not of type 'Element'.");
        }
    }
    function rect(x, y, w, h) {
        var C = g.DOMRectReadOnly || g.DOMRect;
        return typeof C === 'function'
            ? new C(x, y, w, h)
            : { x: x, y: y, width: w, height: h, top: y, left: x, right: x + w, bottom: y + h };
    }
    function now() { return typeof g.performance === 'object' && g.performance.now ? g.performance.now() : Date.now(); }
    function define(name, value) {
        Object.defineProperty(g, name, { value: value, writable: true, configurable: true, enumerable: false });
    }

    var observers = [];          // every IntersectionObserver and ResizeObserver with targets
    var scheduled = false;
    function schedule() {
        if (scheduled || typeof g.setTimeout !== 'function') return;
        scheduled = true;
        g.setTimeout(function () { scheduled = false; tick(); }, 0);
    }
    function track(o) { if (observers.indexOf(o) < 0) observers.push(o); }
    function untrackIfEmpty(o) {
        if (!o._targets.length) { var i = observers.indexOf(o); if (i >= 0) observers.splice(i, 1); }
    }

    // ---- IntersectionObserver
    // "10px 5%" -> [top, right, bottom, left] as {v, pct}.
    function parseMargin(text) {
        var parts = String(text).trim().split(/\s+/).filter(Boolean);
        if (!parts.length) parts = ['0px'];
        if (parts.length > 4) throw new SyntaxError("Failed to construct 'IntersectionObserver': Failed to parse rootMargin '" + text + "'.");
        var out = parts.map(function (p) {
            var m = /^(-?\d*\.?\d+)(px|%)$/.exec(p);
            if (!m) {
                if (p === '0') return { v: 0, pct: false };
                throw new SyntaxError("Failed to construct 'IntersectionObserver': rootMargin must be specified in pixels or percent.");
            }
            return { v: parseFloat(m[1]), pct: m[2] === '%' };
        });
        var t = out[0], r = out[1] || t, b = out[2] || t, l = out[3] || r;
        return [t, r, b, l];
    }
    function IntersectionObserver(callback, options) {
        if (!(this instanceof IntersectionObserver)) throw new TypeError("Failed to construct 'IntersectionObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        requireCallback('IntersectionObserver', callback);
        var o = options || {};
        if (o.root !== undefined && o.root !== null && o.root.nodeType !== 1 && o.root.nodeType !== 9) {
            throw new TypeError("Failed to construct 'IntersectionObserver': Failed to read the 'root' property from 'IntersectionObserverInit': The provided value is not of type '(Document or Element)'.");
        }
        var th = o.threshold === undefined ? [0] : (Array.isArray(o.threshold) ? o.threshold : [o.threshold]);
        th = th.map(Number);
        th.forEach(function (n) {
            if (!(n >= 0 && n <= 1)) throw new RangeError("Failed to construct 'IntersectionObserver': Threshold values must be numbers between 0 and 1.");
        });
        th.sort(function (a, b) { return a - b; });
        if (!th.length) th = [0];
        var marginText = o.rootMargin === undefined ? '0px 0px 0px 0px' : String(o.rootMargin);
        var margin = parseMargin(marginText);
        Object.defineProperty(this, 'root', { value: o.root === undefined ? null : o.root, enumerable: true });
        Object.defineProperty(this, 'rootMargin', { value: marginText, enumerable: true });
        Object.defineProperty(this, 'thresholds', { value: Object.freeze(th), enumerable: true });
        Object.defineProperty(this, '_cb', { value: callback });
        Object.defineProperty(this, '_margin', { value: margin });
        Object.defineProperty(this, '_targets', { value: [] });
        Object.defineProperty(this, '_state', { value: new Map() });
        Object.defineProperty(this, '_queue', { value: [] });
    }
    IntersectionObserver.prototype.observe = function (target) {
        requireElement('IntersectionObserver', target);
        if (this._targets.indexOf(target) >= 0) return;
        this._targets.push(target);
        this._state.set(target, { index: -1, inter: false });
        track(this);
        schedule();
    };
    IntersectionObserver.prototype.unobserve = function (target) {
        var i = this._targets.indexOf(target);
        if (i >= 0) { this._targets.splice(i, 1); this._state.delete(target); }
        untrackIfEmpty(this);
    };
    IntersectionObserver.prototype.disconnect = function () {
        this._targets.length = 0;
        this._state.clear();
        untrackIfEmpty(this);
    };
    IntersectionObserver.prototype.takeRecords = function () { return this._queue.splice(0, this._queue.length); };
    Object.defineProperty(IntersectionObserver.prototype, Symbol.toStringTag, { value: 'IntersectionObserver', configurable: true });
    define('IntersectionObserver', IntersectionObserver);

    function area(r) { return Math.max(0, r.w) * Math.max(0, r.h); }
    function rootBounds(io) {
        var base;
        if (io.root && io.root.nodeType === 1) {
            var b = io.root.getBoundingClientRect();
            base = { x: b.left, y: b.top, w: b.width, h: b.height };
        } else {
            base = { x: 0, y: 0, w: g.innerWidth || 0, h: g.innerHeight || 0 };
        }
        var m = io._margin;
        function px(side, dim) { return m[side].pct ? dim * m[side].v / 100 : m[side].v; }
        var top = px(0, base.h), right = px(1, base.w), bottom = px(2, base.h), left = px(3, base.w);
        return { x: base.x - left, y: base.y - top, w: base.w + left + right, h: base.h + top + bottom };
    }
    function observeIntersections(io) {
        var rb = rootBounds(io), th = io.thresholds, t0 = now();
        io._targets.slice().forEach(function (target) {
            var b = target.getBoundingClientRect();
            var tr = { x: b.left, y: b.top, w: b.width, h: b.height };
            var l = Math.max(tr.x, rb.x), t = Math.max(tr.y, rb.y);
            var r = Math.min(tr.x + tr.w, rb.x + rb.w), bt = Math.min(tr.y + tr.h, rb.y + rb.h);
            // Intersecting, or edge-adjacent, per the spec.
            var inter = l <= r && t <= bt && target.isConnected;
            var ir = inter ? { x: l, y: t, w: r - l, h: bt - t } : { x: 0, y: 0, w: 0, h: 0 };
            var ta = area(tr);
            var ratio = ta > 0 ? Math.min(1, area(ir) / ta) : (inter ? 1 : 0);
            var index = 0;
            if (inter) { while (index < th.length && th[index] <= ratio) index++; }
            var st = io._state.get(target);
            if (!st) return;
            if (index === st.index && inter === st.inter) return;
            st.index = index; st.inter = inter;
            io._queue.push({
                time: t0,
                rootBounds: rect(rb.x, rb.y, rb.w, rb.h),
                boundingClientRect: rect(tr.x, tr.y, tr.w, tr.h),
                intersectionRect: rect(ir.x, ir.y, ir.w, ir.h),
                isIntersecting: inter,
                intersectionRatio: ratio,
                target: target
            });
        });
    }

    // ---- ResizeObserver
    function ResizeObserver(callback) {
        if (!(this instanceof ResizeObserver)) throw new TypeError("Failed to construct 'ResizeObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        requireCallback('ResizeObserver', callback);
        Object.defineProperty(this, '_cb', { value: callback });
        Object.defineProperty(this, '_targets', { value: [] });
        Object.defineProperty(this, '_state', { value: new Map() });
        Object.defineProperty(this, '_queue', { value: [] });
    }
    ResizeObserver.prototype.observe = function (target) {
        requireElement('ResizeObserver', target);
        if (this._targets.indexOf(target) >= 0) return;
        this._targets.push(target);
        this._state.set(target, { w: 0, h: 0 });   // nothing reported yet: a sized element reports at once
        track(this);
        schedule();
    };
    ResizeObserver.prototype.unobserve = function (target) {
        var i = this._targets.indexOf(target);
        if (i >= 0) { this._targets.splice(i, 1); this._state.delete(target); }
        untrackIfEmpty(this);
    };
    ResizeObserver.prototype.disconnect = function () {
        this._targets.length = 0;
        this._state.clear();
        untrackIfEmpty(this);
    };
    Object.defineProperty(ResizeObserver.prototype, Symbol.toStringTag, { value: 'ResizeObserver', configurable: true });
    define('ResizeObserver', ResizeObserver);

    function px(v) { var n = parseFloat(v); return isNaN(n) ? 0 : n; }
    function observeSizes(ro) {
        ro._targets.slice().forEach(function (target) {
            var cs = g.getComputedStyle(target);
            var w = px(cs.width), h = px(cs.height);
            var st = ro._state.get(target);
            if (!st || (st.w === w && st.h === h)) return;
            st.w = w; st.h = h;
            var bw = target.offsetWidth || 0, bh = target.offsetHeight || 0;
            var size = function (i, b) { return Object.freeze([Object.freeze({ inlineSize: i, blockSize: b })]); };
            ro._queue.push({
                target: target,
                contentRect: rect(px(cs.paddingLeft), px(cs.paddingTop), w, h),
                borderBoxSize: size(bw, bh),
                contentBoxSize: size(w, h),
                devicePixelContentBoxSize: size(w, h)
            });
        });
    }

    // ---- delivery
    var ticking = false;
    function tick() {
        if (ticking) return 0;
        ticking = true;
        try {
            // A callback can observe more or change layout-independent state;
            // what it changes is seen by the next tick, not this one.
            observers.slice().forEach(function (o) {
                if (o instanceof IntersectionObserver) observeIntersections(o); else observeSizes(o);
            });
            observers.slice().forEach(function (o) {
                if (!o._queue.length) return;
                var entries = o._queue.splice(0, o._queue.length);
                try { o._cb.call(o, entries, o); } catch (e) { report(e); }
            });
        } finally {
            ticking = false;
        }
        return observers.length;
    }
    Object.defineProperty(g, '__rkObserversTick', { value: tick, configurable: true, writable: true, enumerable: false });
})(typeof globalThis === 'object' ? globalThis : this);
