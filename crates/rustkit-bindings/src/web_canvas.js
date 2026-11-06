// Canvas 2D (HTML §4.12.5): HTMLCanvasElement.width/height/getContext and
// CanvasRenderingContext2D, CanvasGradient, TextMetrics. The drawing is
// recorded on the Rust side (web_canvas.rs, rustkit-canvas), one context per
// canvas, and painted by the engine in the canvas's box. The attribute
// values script reads back (fillStyle, font, lineWidth, ...) are kept here,
// saved and restored with the Rust state.
//
// Phase 1 is the subset charting libraries draw with: state, transforms,
// rects, paths, clip, solid colours and linear gradients, line styles and
// dashes, text and measureText. Members not listed below are absent, not
// stubbed: no createRadialGradient/createPattern, drawImage, shadows,
// globalCompositeOperation, getImageData/putImageData, isPointInPath,
// toDataURL. getContext of any other kind is null.
(function (g) {
    var C = g.__rustkit_canvas, slot = g.__rustkit_canvas_slot;
    delete g.__rustkit_canvas;
    delete g.__rustkit_canvas_slot;
    var Canvas = g.HTMLCanvasElement;
    if (typeof C !== 'function' || typeof slot !== 'function' || typeof Canvas !== 'function') return;

    function illegal() { throw new TypeError('Illegal constructor'); }
    function iface(name) {
        var ctor = function () { illegal(); };
        Object.defineProperty(ctor, 'name', { value: name });
        Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
        Object.defineProperty(g, name, { value: ctor, writable: true, configurable: true, enumerable: false });
        return ctor;
    }
    function method(proto, name, fn) {
        Object.defineProperty(fn, 'name', { value: name });
        Object.defineProperty(proto, name, { value: fn, writable: true, configurable: true, enumerable: true });
    }
    function accessor(proto, name, get, set) {
        Object.defineProperty(proto, name, { get: get, set: set, configurable: true, enumerable: true });
    }
    function domError(name, message) {
        return typeof g.DOMException === 'function' ? new g.DOMException(message, name) : new Error(message);
    }
    function need(n, args, iface, member) {
        if (args.length < n) {
            throw new TypeError("Failed to execute '" + member + "' on '" + iface + "': " + n +
                ' argument' + (n === 1 ? '' : 's') + ' required, but only ' + args.length + ' present.');
        }
    }

    // ---- HTMLCanvasElement: width/height reflect the attributes (default
    // 300x150); setting either, even to the value it has, clears the bitmap
    // and resets the context.
    var CTX = new WeakMap();
    function dimension(el, name, fallback) {
        var raw = el.getAttribute(name);
        if (raw === null) return fallback;
        var m = /^[\t\n\f\r ]*\+?(\d+)/.exec(raw);
        if (!m) return fallback;
        var n = Number(m[1]);
        return n > 2147483647 ? fallback : n;
    }
    function resized(el) {
        var ctx = CTX.get(el);
        if (!ctx) return;
        var d = DATA.get(ctx);
        C(d.gen, d.id, 'resize', el.width, el.height);
        d.st = initial();
        d.stack = [];
    }
    [['width', 300], ['height', 150]].forEach(function (p) {
        accessor(Canvas.prototype, p[0], function () {
            return dimension(this, p[0], p[1]);
        }, function (v) {
            var n = Number(v) >>> 0;
            if (n > 2147483647) n = p[1];
            this.setAttribute(p[0], String(n));
            resized(this);
        });
    });

    var Ctx2D = iface('CanvasRenderingContext2D');
    var Gradient = iface('CanvasGradient');
    var Metrics = iface('TextMetrics');
    var DATA = new WeakMap();
    // A CanvasGradient's geometry and stops live here, on the script side,
    // and go with it when script lets it go; a fill or stroke style set to
    // it hands the Rust side a copy (PR #905 R1: no gradient table beside
    // the recording).
    var GRAD = new WeakMap();

    method(Canvas.prototype, 'getContext', function getContext(kind) {
        need(1, arguments, 'HTMLCanvasElement', 'getContext');
        var s = slot(this);
        var have = CTX.get(this);
        if (String(kind) !== '2d') return null;
        if (have) return have;
        var ctx = Object.create(Ctx2D.prototype);
        DATA.set(ctx, { el: this, gen: s[0], id: s[1], st: initial(), stack: [] });
        CTX.set(this, ctx);
        C(s[0], s[1], 'create', this.width, this.height);
        return ctx;
    });

    function initial() {
        return {
            fillStyle: '#000000', strokeStyle: '#000000', lineWidth: 1, lineCap: 'butt', lineJoin: 'miter',
            miterLimit: 10, lineDash: [], lineDashOffset: 0, font: '10px sans-serif', textAlign: 'start',
            textBaseline: 'alphabetic', globalAlpha: 1
        };
    }
    function data(o) {
        var d = o != null ? DATA.get(o) : undefined;
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }
    function call(d, op, a, b, c, e, f, h, i, j) {
        return C(d.gen, d.id, op, a, b, c, e, f, h, i, j);
    }
    var P = Ctx2D.prototype;
    accessor(P, 'canvas', function () { return data(this).el; });

    // ---- State
    method(P, 'save', function save() {
        var d = data(this), st = {};
        for (var k in d.st) st[k] = d.st[k];
        d.stack.push(st);
        call(d, 'save');
    });
    method(P, 'restore', function restore() {
        var d = data(this);
        if (!d.stack.length) return;
        d.st = d.stack.pop();
        call(d, 'restore');
    });

    // ---- Transforms (non-finite arguments are ignored, in Rust)
    [['scale', 2], ['rotate', 1], ['translate', 2], ['transform', 6]].forEach(function (m) {
        method(P, m[0], function () {
            need(m[1], arguments, 'CanvasRenderingContext2D', m[0]);
            var d = data(this), a = arguments;
            call(d, m[0], +a[0], +a[1], +a[2], +a[3], +a[4], +a[5]);
        });
    });
    method(P, 'setTransform', function setTransform(a, b, c, e, f, h) {
        var d = data(this);
        if (arguments.length === 0) { call(d, 'resetTransform'); return; }
        if (arguments.length === 1 && a !== null && typeof a === 'object') {
            var m = a;
            a = 'a' in m ? m.a : ('m11' in m ? m.m11 : 1); b = 'b' in m ? m.b : ('m12' in m ? m.m12 : 0);
            c = 'c' in m ? m.c : ('m21' in m ? m.m21 : 0); e = 'd' in m ? m.d : ('m22' in m ? m.m22 : 1);
            f = 'e' in m ? m.e : ('m41' in m ? m.m41 : 0); h = 'f' in m ? m.f : ('m42' in m ? m.m42 : 0);
        } else {
            need(6, arguments, 'CanvasRenderingContext2D', 'setTransform');
        }
        call(d, 'setTransform', +a, +b, +c, +e, +f, +h);
    });
    method(P, 'resetTransform', function resetTransform() { call(data(this), 'resetTransform'); });

    // ---- Fill and stroke styles: a colour string or a CanvasGradient
    function style(name, colorOp, gradientOp) {
        accessor(P, name, function () { return data(this).st[name]; }, function (v) {
            var d = data(this);
            var gi = v !== null && typeof v === 'object' ? GRAD.get(v) : undefined;
            if (gi !== undefined) {
                call(d, gradientOp, gi.x0, gi.y0, gi.x1, gi.y1, gi.stops.join('|'));
                d.st[name] = v;
                return;
            }
            if (typeof v === 'object' && v !== null) return;
            var out = call(d, colorOp, String(v));
            if (typeof out === 'string') d.st[name] = out;
        });
    }
    style('fillStyle', 'fillColor', 'fillGradient');
    style('strokeStyle', 'strokeColor', 'strokeGradient');
    method(P, 'createLinearGradient', function createLinearGradient(x0, y0, x1, y1) {
        need(4, arguments, 'CanvasRenderingContext2D', 'createLinearGradient');
        data(this);
        var a = [+x0, +y0, +x1, +y1];
        for (var i = 0; i < 4; i++) {
            if (!isFinite(a[i])) throw new TypeError("Failed to execute 'createLinearGradient' on 'CanvasRenderingContext2D': The provided double value is non-finite.");
        }
        var grad = Object.create(Gradient.prototype);
        GRAD.set(grad, { x0: a[0], y0: a[1], x1: a[2], y1: a[3], stops: [] });
        return grad;
    });
    method(Gradient.prototype, 'addColorStop', function addColorStop(offset, color) {
        need(2, arguments, 'CanvasGradient', 'addColorStop');
        var gi = GRAD.get(this);
        if (gi === undefined) throw new TypeError('Illegal invocation');
        var out = C(-1, -1, 'gradientStop', +offset, String(color));
        if (out === 'IndexSizeError') throw domError('IndexSizeError', "Failed to execute 'addColorStop' on 'CanvasGradient': The provided value (" + offset + ') is outside the range (0.0, 1.0).');
        if (out === 'SyntaxError') throw domError('SyntaxError', "Failed to execute 'addColorStop' on 'CanvasGradient': The value provided ('" + color + "') could not be parsed as a color.");
        gi.stops.push(+offset, out);
    });

    // ---- Line styles
    function numeric(name, valid) {
        accessor(P, name, function () { return data(this).st[name]; }, function (v) {
            var d = data(this), n = +v;
            if (!valid(n)) return;
            d.st[name] = n;
            call(d, name, n);
        });
    }
    numeric('lineWidth', function (n) { return isFinite(n) && n > 0; });
    numeric('miterLimit', function (n) { return isFinite(n) && n > 0; });
    numeric('lineDashOffset', function (n) { return isFinite(n); });
    numeric('globalAlpha', function (n) { return isFinite(n) && n >= 0 && n <= 1; });
    function keyword(name, values) {
        accessor(P, name, function () { return data(this).st[name]; }, function (v) {
            var d = data(this);
            v = String(v);
            if (values.indexOf(v) < 0) return;
            d.st[name] = v;
            call(d, name, v);
        });
    }
    keyword('lineCap', ['butt', 'round', 'square']);
    keyword('lineJoin', ['round', 'bevel', 'miter']);
    keyword('textAlign', ['start', 'end', 'left', 'right', 'center']);
    keyword('textBaseline', ['top', 'hanging', 'middle', 'alphabetic', 'ideographic', 'bottom']);
    method(P, 'setLineDash', function setLineDash(segments) {
        need(1, arguments, 'CanvasRenderingContext2D', 'setLineDash');
        var d = data(this);
        if (segments === null || typeof segments !== 'object' || typeof segments.length !== 'number') {
            throw new TypeError("Failed to execute 'setLineDash' on 'CanvasRenderingContext2D': The provided value cannot be converted to a sequence.");
        }
        var list = [];
        for (var i = 0; i < segments.length; i++) {
            var n = +segments[i];
            if (!isFinite(n) || n < 0) return;
            list.push(n);
        }
        if (list.length % 2) list = list.concat(list);
        d.st.lineDash = list;
        call(d, 'lineDash', list.join(','));
    });
    method(P, 'getLineDash', function getLineDash() { return data(this).st.lineDash.slice(); });

    // ---- Text
    accessor(P, 'font', function () { return data(this).st.font; }, function (v) {
        var d = data(this);
        var out = call(d, 'font', String(v));
        if (typeof out === 'string') d.st.font = out;
    });
    ['fillText', 'strokeText'].forEach(function (name) {
        method(P, name, function (text, x, y, maxWidth) {
            need(3, arguments, 'CanvasRenderingContext2D', name);
            var d = data(this);
            if (arguments.length > 3 && maxWidth !== undefined) call(d, name, String(text), +x, +y, +maxWidth);
            else call(d, name, String(text), +x, +y);
        });
    });
    method(P, 'measureText', function measureText(text) {
        need(1, arguments, 'CanvasRenderingContext2D', 'measureText');
        var d = data(this);
        var p = String(call(d, 'measure', String(text)) || '0 0 0').split(' ');
        var w = +p[0], asc = +p[1], desc = +p[2];
        var align = d.st.textAlign;
        var left = align === 'center' ? w / 2 : (align === 'right' || align === 'end') ? w : 0;
        var m = Object.create(Metrics.prototype);
        var values = {
            width: w, actualBoundingBoxLeft: left, actualBoundingBoxRight: w - left,
            fontBoundingBoxAscent: asc, fontBoundingBoxDescent: desc,
            actualBoundingBoxAscent: asc, actualBoundingBoxDescent: desc
        };
        Object.keys(values).forEach(function (k) {
            Object.defineProperty(m, k, { value: values[k], enumerable: true });
        });
        return m;
    });

    // ---- Rects
    ['fillRect', 'strokeRect', 'clearRect'].forEach(function (name) {
        method(P, name, function (x, y, w, h) {
            need(4, arguments, 'CanvasRenderingContext2D', name);
            call(data(this), name, +x, +y, +w, +h);
        });
    });

    // ---- Paths
    [['beginPath', 0], ['closePath', 0], ['moveTo', 2], ['lineTo', 2], ['quadraticCurveTo', 4],
     ['bezierCurveTo', 6], ['rect', 4]].forEach(function (m) {
        method(P, m[0], function () {
            need(m[1], arguments, 'CanvasRenderingContext2D', m[0]);
            var a = arguments;
            call(data(this), m[0], +a[0], +a[1], +a[2], +a[3], +a[4], +a[5]);
        });
    });
    function indexSize(out, name) {
        if (out === 'IndexSizeError') {
            throw domError('IndexSizeError', "Failed to execute '" + name + "' on 'CanvasRenderingContext2D': The radius provided is negative.");
        }
    }
    method(P, 'arc', function arc(x, y, r, a0, a1, ccw) {
        need(5, arguments, 'CanvasRenderingContext2D', 'arc');
        indexSize(call(data(this), 'arc', +x, +y, +r, +a0, +a1, !!ccw), 'arc');
    });
    method(P, 'arcTo', function arcTo(x1, y1, x2, y2, r) {
        need(5, arguments, 'CanvasRenderingContext2D', 'arcTo');
        indexSize(call(data(this), 'arcTo', +x1, +y1, +x2, +y2, +r), 'arcTo');
    });
    method(P, 'ellipse', function ellipse(x, y, rx, ry, rot, a0, a1, ccw) {
        need(7, arguments, 'CanvasRenderingContext2D', 'ellipse');
        indexSize(call(data(this), 'ellipse', +x, +y, +rx, +ry, +rot, +a0, +a1, !!ccw), 'ellipse');
    });
    function rule(v) {
        if (v === undefined) return 'nonzero';
        v = String(v);
        if (v !== 'nonzero' && v !== 'evenodd') {
            throw new TypeError("The provided value '" + v + "' is not a valid enum value of type CanvasFillRule.");
        }
        return v;
    }
    method(P, 'fill', function fill(r) { var d = data(this); call(d, 'fill', rule(r)); });
    method(P, 'stroke', function stroke() { call(data(this), 'stroke'); });
    method(P, 'clip', function clip(r) { var d = data(this); call(d, 'clip', rule(r)); });
})(typeof globalThis === 'object' ? globalThis : this);
