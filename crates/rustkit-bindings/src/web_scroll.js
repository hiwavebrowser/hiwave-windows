// Scroll position for page script (CSSOM View §6, §7): window.scrollX/scrollY,
// scrollTo/scroll/scrollBy, Element.scrollTop/scrollLeft/scrollTo/scrollBy,
// scrollIntoView and document.scrollingElement.
//
// The window's scroll offset is the engine's (view.scroll_offset): reads
// answer from the position it last published, and a scroll writes through to
// the host, which clamps it to the document and hands it to the engine when
// script settles. Element scroll offsets are not rendered (the engine has no
// per-element scrolling): they are stored and clamped to the element's
// scroll size, so code that sets then reads them sees consistent numbers.
// `behavior` is ignored (always instant). A scroll fires one `scroll` event
// on the document (it bubbles to the window), from a timer, not synchronously.
(function (g) {
    var read = g.__rustkit_scroll_state, to = g.__rustkit_scroll_to;
    delete g.__rustkit_scroll_state;
    delete g.__rustkit_scroll_to;
    if (typeof read !== 'function' || typeof to !== 'function') return;
    var document = g.document;
    if (!document || !g.Element) return;

    function pos() {
        var p = read().split(' ');
        return { x: +p[0], y: +p[1], mx: +p[2], my: +p[3] };
    }

    // scrollX and friends are [Replaceable]: assigning replaces the property.
    function replaceable(name, get) {
        Object.defineProperty(g, name, {
            get: get,
            set: function (v) {
                Object.defineProperty(g, name, { value: v, writable: true, configurable: true, enumerable: true });
            },
            configurable: true, enumerable: true
        });
    }
    replaceable('scrollX', function () { return pos().x; });
    replaceable('pageXOffset', function () { return pos().x; });
    replaceable('scrollY', function () { return pos().y; });
    replaceable('pageYOffset', function () { return pos().y; });

    var queued = false;
    function scrolled(before) {
        var after = pos();
        if (after.x === before.x && after.y === before.y) return;
        if (queued || typeof g.setTimeout !== 'function' || typeof g.Event !== 'function') return;
        queued = true;
        g.setTimeout(function () {
            queued = false;
            try { document.dispatchEvent(new g.Event('scroll', { bubbles: true })); } catch (_) {}
        }, 0);
    }

    // (x, y) or ({ left, top, behavior }); anything else keeps that axis.
    function target(args) {
        var a = args[0], x, y;
        if (a !== null && typeof a === 'object') { x = a.left; y = a.top; }
        else { x = args[0]; y = args[1]; }
        return [typeof x === 'number' ? x : NaN, typeof y === 'number' ? y : NaN];
    }
    function windowScroll(relative) {
        return function () {
            var t = target(arguments), p = pos();
            if (relative) {
                t[0] = isNaN(t[0]) ? NaN : p.x + t[0];
                t[1] = isNaN(t[1]) ? NaN : p.y + t[1];
            }
            to(t[0], t[1]);
            scrolled(p);
        };
    }
    function method(obj, name, fn) {
        Object.defineProperty(obj, name, { value: fn, writable: true, configurable: true, enumerable: true });
    }
    method(g, 'scrollTo', windowScroll(false));
    method(g, 'scroll', windowScroll(false));
    method(g, 'scrollBy', windowScroll(true));

    // ---- elements
    var E = g.Element.prototype;
    var offsets = new WeakMap();
    function isRoot(el) { return el === document.documentElement; }
    function own(el) {
        var o = offsets.get(el);
        if (!o) { o = { x: 0, y: 0 }; offsets.set(el, o); }
        return o;
    }
    function clamp(v, max) { return Math.min(Math.max(0, v), Math.max(0, max)); }
    function setOwn(el, x, y) {
        var o = own(el);
        if (!isNaN(x)) o.x = clamp(x, el.scrollWidth - el.clientWidth);
        if (!isNaN(y)) o.y = clamp(y, el.scrollHeight - el.clientHeight);
    }
    function accessor(name, axis) {
        Object.defineProperty(E, name, {
            get: function () { return isRoot(this) ? Math.round(pos()[axis]) : Math.round(own(this)[axis]); },
            set: function (v) {
                v = +v;
                if (isNaN(v)) return;
                if (isRoot(this)) {
                    var p = pos();
                    to(axis === 'x' ? v : NaN, axis === 'y' ? v : NaN);
                    scrolled(p);
                } else {
                    setOwn(this, axis === 'x' ? v : NaN, axis === 'y' ? v : NaN);
                }
            },
            configurable: true, enumerable: true
        });
    }
    accessor('scrollTop', 'y');
    accessor('scrollLeft', 'x');
    function elementScroll(relative) {
        return function () {
            var t = target(arguments);
            if (isRoot(this)) {
                var p = pos();
                if (relative) {
                    t[0] = isNaN(t[0]) ? NaN : p.x + t[0];
                    t[1] = isNaN(t[1]) ? NaN : p.y + t[1];
                }
                to(t[0], t[1]);
                scrolled(p);
                return;
            }
            var o = own(this);
            setOwn(this, relative && !isNaN(t[0]) ? o.x + t[0] : t[0], relative && !isNaN(t[1]) ? o.y + t[1] : t[1]);
        };
    }
    method(E, 'scrollTo', elementScroll(false));
    method(E, 'scroll', elementScroll(false));
    method(E, 'scrollBy', elementScroll(true));

    // Where to scroll so that the edge `start` (relative to the viewport) of
    // something `size` long shows in a viewport `view` long, from `cur`.
    function align(start, size, view, cur, mode) {
        switch (mode) {
            case 'start': return cur + start;
            case 'end': return cur + start + size - view;
            case 'center': return cur + start + size / 2 - view / 2;
            default:
                if (start >= 0 && start + size <= view) return cur;
                if (size > view || start < 0) return cur + start;
                return cur + start + size - view;
        }
    }
    method(E, 'scrollIntoView', function (arg) {
        var block = 'start', inline = 'nearest';
        if (arg === false) block = 'end';
        else if (arg !== null && typeof arg === 'object') {
            if (arg.block) block = arg.block;
            if (arg.inline) inline = arg.inline;
        }
        var r = this.getBoundingClientRect(), p = pos();
        to(align(r.left, r.width, g.innerWidth, p.x, inline), align(r.top, r.height, g.innerHeight, p.y, block));
        scrolled(p);
    });

    // The engine calls this when the USER scrolled (wheel, keys, scrollbar):
    // the scroll event fires at once, not from a timer.
    Object.defineProperty(g, '__rkUserScrolled', {
        value: function () {
            try { document.dispatchEvent(new g.Event('scroll', { bubbles: true })); } catch (_) {}
        },
        configurable: true, writable: true, enumerable: false
    });

    var D = g.Document && g.Document.prototype;
    if (D && D.scrollingElement === undefined) {
        Object.defineProperty(D, 'scrollingElement', {
            get: function () { return document.documentElement; },
            configurable: true, enumerable: true
        });
    }
})(typeof globalThis === 'object' ? globalThis : this);
