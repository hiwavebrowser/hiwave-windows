// Canvas 2D (HTML §4.12.5): HTMLCanvasElement.width/height/getContext and
// CanvasRenderingContext2D, CanvasGradient, TextMetrics. The drawing is
// recorded on the Rust side (web_canvas.rs, rustkit-canvas), one context per
// canvas, and painted by the engine in the canvas's box. The attribute
// values script reads back (fillStyle, font, lineWidth, ...) are kept here,
// saved and restored with the Rust state.
//
// Phase 1 is the subset charting libraries draw with: state, transforms,
// rects, paths, clip, solid colours and linear gradients, line styles and
// dashes, text and measureText; STR-7 added the raster's members
// (getImageData, putImageData, createImageData, ImageData, drawImage of a
// canvas or a loaded <img>, createPattern, createRadialGradient). Members
// not listed are absent, not stubbed: no shadows, globalCompositeOperation,
// isPointInPath, toDataURL. getContext of any other kind is null.
(function (g) {
    var C = __rk.canvasCall, slot = __rk.canvasSlot;
    var Canvas = g.HTMLCanvasElement;
    if (typeof C !== 'function' || typeof slot !== 'function' || typeof Canvas !== 'function') return;

    // The built-ins this script keeps its records with, as they were when
    // the realm was made: handed over by the lazy installer, which took
    // them then (a frame's realm installs this script at first canvas use,
    // after page script has run), or taken here when this runs at realm
    // creation itself.
    var B = __rk.builtins || (function () {
        var b = Object.create(null), p = WeakMap.prototype;
        b.WeakMap = WeakMap; b.wmGet = p.get; b.wmSet = p.set; b.wmHas = p.has; b.wmDelete = p.delete;
        b.defineProperty = Object.defineProperty; b.create = Object.create;
        b.trunc = Math.trunc; b.abs = Math.abs; b.isFinite = isFinite;
        b.String = String; b.TypeError = TypeError; b.RangeError = RangeError;
        b.apply = Reflect.apply;
        b.bufferOf = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8ClampedArray.prototype), 'buffer').get;
        var resizable = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'resizable');
        b.resizableOf = resizable && resizable.get;
        return b;
    })();
    var WM = B.WeakMap, defineOwn = B.defineProperty, newRecord = B.create;
    // The TypeError the realm had when this script was installed: a page that
    // replaces the global does not change what the canvas members throw.
    var TypeErrorAtInstall = B.TypeError;
    function valueOnly(v) { var d = newRecord(null); d.value = v; return d; }
    // A WeakMap whose methods are its own properties: the script's records
    // are found by one direct call, whatever a page puts on
    // WeakMap.prototype.
    function ownMap() {
        var m = new WM();
        defineOwn(m, 'get', valueOnly(B.wmGet));
        defineOwn(m, 'set', valueOnly(B.wmSet));
        defineOwn(m, 'has', valueOnly(B.wmHas));
        defineOwn(m, 'delete', valueOnly(B.wmDelete));
        return m;
    }

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
        if (args.length < n) tooFew(n, args.length, iface, member);
    }
    function tooFew(n, got, iface, member) {
        throw new TypeErrorAtInstall("Failed to execute '" + member + "' on '" + iface + "': " + n +
            ' argument' + (n === 1 ? '' : 's') + ' required, but only ' + got + ' present.');
    }

    // ---- HTMLCanvasElement: width/height reflect the attributes (default
    // 300x150); setting either, even to the value it has, clears the bitmap
    // and resets the context.
    var CTX = ownMap();
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
    var DATA = ownMap();
    // A CanvasGradient's geometry and stops live here, on the script side,
    // and go with it when script lets it go; a fill or stroke style set to
    // it hands the Rust side a copy (PR #905 R1: no gradient table beside
    // the recording).
    var GRAD = ownMap();
    // A CanvasPattern's source (a canvas's NodeId or an image's URL) and
    // repetition; its pixels are read when it is set as a style.
    var Pattern = iface('CanvasPattern');
    var PAT = ownMap();

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
    function illegalCall() { throw new TypeErrorAtInstall('Illegal invocation'); }
    // CANVAS-SHIM: the members MotionMark's canvas tests and charting
    // libraries call thousands of times a frame look their record up
    // inline (`DATA.get(this)`; a WeakMap answers undefined for a
    // non-object), call the host directly with the arguments the operation
    // reads, and count their arguments in strict mode: a sloppy function
    // that reads `arguments` makes a mapped arguments object on every call,
    // the dearest part of a call in Boa. The same checks, in Chrome 148's
    // order (the receiver, then the argument count, then each argument's
    // conversion, left to right; tools/parity_oracle/canvas_shim_chrome.json),
    // and the same host operations.
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
    method(P, 'scale', function (x, y) {
        'use strict';
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        if (arguments.length < 2) tooFew(2, arguments.length, 'CanvasRenderingContext2D', 'scale');
        C(d.gen, d.id, 'scale', +x, +y);
    });
    method(P, 'rotate', function (a) {
        'use strict';
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        if (arguments.length < 1) tooFew(1, arguments.length, 'CanvasRenderingContext2D', 'rotate');
        C(d.gen, d.id, 'rotate', +a);
    });
    method(P, 'translate', function (x, y) {
        'use strict';
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        if (arguments.length < 2) tooFew(2, arguments.length, 'CanvasRenderingContext2D', 'translate');
        C(d.gen, d.id, 'translate', +x, +y);
    });
    method(P, 'transform', function (a, b, c, e, f, h) {
        'use strict';
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        if (arguments.length < 6) tooFew(6, arguments.length, 'CanvasRenderingContext2D', 'transform');
        C(d.gen, d.id, 'transform', +a, +b, +c, +e, +f, +h);
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
            var d = DATA.get(this), out;
            if (d === undefined) illegalCall();
            if (typeof v === 'string') {
                out = C(d.gen, d.id, colorOp, v);
                if (typeof out === 'string') d.st[name] = out;
                return;
            }
            var gi = v !== null && typeof v === 'object' ? GRAD.get(v) : undefined;
            if (gi !== undefined) {
                if (gi.radial) call(d, gradientOp.replace('Gradient', 'Radial'), gi.x0, gi.y0, gi.r0, gi.x1, gi.y1, gi.r1, gi.stops.join('|'));
                else call(d, gradientOp, gi.x0, gi.y0, gi.x1, gi.y1, gi.stops.join('|'));
                d.st[name] = v;
                return;
            }
            var pi = v !== null && typeof v === 'object' ? PAT.get(v) : undefined;
            if (pi !== undefined) {
                call(d, gradientOp.replace('Gradient', 'Pattern'), pi.kind, pi.src, pi.repetition);
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
    method(P, 'createRadialGradient', function createRadialGradient(x0, y0, r0, x1, y1, r1) {
        need(6, arguments, 'CanvasRenderingContext2D', 'createRadialGradient');
        data(this);
        var a = [+x0, +y0, +r0, +x1, +y1, +r1];
        for (var i = 0; i < 6; i++) {
            if (!isFinite(a[i])) throw new TypeError("Failed to execute 'createRadialGradient' on 'CanvasRenderingContext2D': The provided double value is non-finite.");
        }
        if (a[2] < 0 || a[5] < 0) {
            throw domError('IndexSizeError', "Failed to execute 'createRadialGradient' on 'CanvasRenderingContext2D': The " + (a[2] < 0 ? 'r0' : 'r1') + ' provided is less than 0.');
        }
        var grad = Object.create(Gradient.prototype);
        GRAD.set(grad, { radial: true, x0: a[0], y0: a[1], r0: a[2], x1: a[3], y1: a[4], r1: a[5], stops: [] });
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
            var d = DATA.get(this);
            if (d === undefined) illegalCall();
            var n = +v;
            if (!valid(n)) return;
            d.st[name] = n;
            C(d.gen, d.id, name, n);
        });
    }
    numeric('lineWidth', function (n) { return isFinite(n) && n > 0; });
    numeric('miterLimit', function (n) { return isFinite(n) && n > 0; });
    numeric('lineDashOffset', function (n) { return isFinite(n); });
    numeric('globalAlpha', function (n) { return isFinite(n) && n >= 0 && n <= 1; });
    function keyword(name, values) {
        accessor(P, name, function () { return data(this).st[name]; }, function (v) {
            var d = DATA.get(this);
            if (d === undefined) illegalCall();
            v = String(v);
            if (values.indexOf(v) < 0) return;
            d.st[name] = v;
            C(d.gen, d.id, name, v);
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
            'use strict';
            var d = DATA.get(this);
            if (d === undefined) illegalCall();
            if (arguments.length < 4) tooFew(4, arguments.length, 'CanvasRenderingContext2D', name);
            C(d.gen, d.id, name, +x, +y, +w, +h);
        });
    });

    // ---- Paths
    ['beginPath', 'closePath'].forEach(function (name) {
        method(P, name, function () {
            var d = DATA.get(this);
            if (d === undefined) illegalCall();
            C(d.gen, d.id, name);
        });
    });
    ['moveTo', 'lineTo'].forEach(function (name) {
        method(P, name, function (x, y) {
            'use strict';
            var d = DATA.get(this);
            if (d === undefined) illegalCall();
            if (arguments.length < 2) tooFew(2, arguments.length, 'CanvasRenderingContext2D', name);
            C(d.gen, d.id, name, +x, +y);
        });
    });
    method(P, 'quadraticCurveTo', function (a, b, c, e) {
        'use strict';
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        if (arguments.length < 4) tooFew(4, arguments.length, 'CanvasRenderingContext2D', 'quadraticCurveTo');
        C(d.gen, d.id, 'quadraticCurveTo', +a, +b, +c, +e);
    });
    method(P, 'bezierCurveTo', function (a, b, c, e, f, h) {
        'use strict';
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        if (arguments.length < 6) tooFew(6, arguments.length, 'CanvasRenderingContext2D', 'bezierCurveTo');
        C(d.gen, d.id, 'bezierCurveTo', +a, +b, +c, +e, +f, +h);
    });
    method(P, 'rect', function (x, y, w, h) {
        'use strict';
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        if (arguments.length < 4) tooFew(4, arguments.length, 'CanvasRenderingContext2D', 'rect');
        C(d.gen, d.id, 'rect', +x, +y, +w, +h);
    });
    function indexSize(out, name) {
        if (out === 'IndexSizeError') {
            throw domError('IndexSizeError', "Failed to execute '" + name + "' on 'CanvasRenderingContext2D': The radius provided is negative.");
        }
    }
    method(P, 'arc', function arc(x, y, r, a0, a1, ccw) {
        'use strict';
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        if (arguments.length < 5) tooFew(5, arguments.length, 'CanvasRenderingContext2D', 'arc');
        if (C(d.gen, d.id, 'arc', +x, +y, +r, +a0, +a1, !!ccw) === 'IndexSizeError') indexSize('IndexSizeError', 'arc');
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
    method(P, 'fill', function fill(r) {
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        C(d.gen, d.id, 'fill', r === undefined ? 'nonzero' : rule(r));
    });
    method(P, 'stroke', function stroke() {
        var d = DATA.get(this);
        if (d === undefined) illegalCall();
        C(d.gen, d.id, 'stroke');
    });
    method(P, 'clip', function clip(r) { var d = data(this); call(d, 'clip', rule(r)); });

    // ---- Pixels: the bitmap rasterized behind the recording (STR-7).
    // An ImageData's bytes are a real Uint8ClampedArray that the host fills
    // and reads whole (`pixelsOut`, `pixelsIn`): no loop over pixels runs
    // here. What these paths use is the realm's as it was made (`B`), and
    // nothing is read from the array itself, so a page that replaced
    // Uint8ClampedArray, its prototype or WeakMap's methods changes nothing.
    var pixelsOut = __rk.pixelsOut, pixelsIn = __rk.pixelsIn;
    // The records, by ImageData. The engine reads sizes and bytes from
    // here, never from the object's properties.
    var IMG = ownMap();
    var trunc = B.trunc, absOf = B.abs, finite = B.isFinite, Text = B.String, TypeErr = B.TypeError, RangeErr = B.RangeError;
    // The bytes of the largest read Chrome 148 allows, which is also the
    // largest bitmap a canvas keeps (16384 x 16384, RGBA: 1 GiB; a larger
    // read is a RangeError there and here). A read of that size holds the
    // bytes twice while the host copies them into the array.
    var MAX_BYTES = 16384 * 16384 * 4;
    function imgOf(o) { return IMG.get(o); }
    function keepImg(o, w, h, bytes) {
        var v = newRecord(null);
        v.width = w; v.height = h; v.data = bytes;
        IMG.set(o, v);
        // As in Chrome, the array is also an own property of the object
        // (read-only, enumerable, configurable), in front of the
        // prototype's getter: `image.data[i]` in a loop is a plain read.
        var own = newRecord(null);
        own.value = bytes; own.writable = false; own.enumerable = true; own.configurable = true;
        defineOwn(o, 'data', own);
        return o;
    }
    function sizeError(which) {
        return domError('IndexSizeError', "Failed to construct 'ImageData': The " + which + ' is zero or not a number.');
    }
    // A new array of `n` zero bytes; past what a canvas can hold, a RangeError.
    function zeros(n, member) {
        if (!(n <= MAX_BYTES)) throw new RangeErr("Failed to " + member + ": Out of memory at ImageData creation.");
        return pixelsOut(n, true);
    }
    // ImageDataSettings: only sRGB pixels are kept, so `colorSpace` is
    // checked and the answer is always 'srgb'.
    function settings(s, member) {
        if (s === undefined || s === null) return;
        if (typeof s !== 'object' && typeof s !== 'function') throw new TypeErr("Failed to " + member + ": The provided value is not of type 'ImageDataSettings'.");
        var space = s.colorSpace;
        if (space === undefined) return;
        space = Text(space);
        if (space !== 'srgb' && space !== 'display-p3') {
            throw new TypeErr("Failed to " + member + ": The provided value '" + space + "' is not a valid enum value of type PredefinedColorSpace.");
        }
    }
    // new ImageData(width, height[, settings]) or
    // new ImageData(data, width[, height[, settings]]).
    var Image = function ImageData(a, b, c, e) {
        if (!new.target) throw new TypeErr("Failed to construct 'ImageData': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        if (arguments.length < 2) throw new TypeErr("Failed to construct 'ImageData': 2 arguments required, but only " + arguments.length + ' present.');
        var w, h, bytes, n = pixelsIn(a, false);
        if (n !== -2) {
            // A Uint8ClampedArray, kept as it is: the page's writes to it are this ImageData's.
            // One on a buffer that can change its length is not taken (Chrome: TypeError).
            if (n !== -1 && B.resizableOf && B.apply(B.resizableOf, B.apply(B.bufferOf, a, []), [])) {
                throw new TypeErr("Failed to construct 'ImageData': The provided ArrayBufferView value must not be resizable.");
            }
            if (n <= 0 || n % 4) {
                throw domError('InvalidStateError', "Failed to construct 'ImageData': The input data has zero elements or its length is not a multiple of 4.");
            }
            w = b >>> 0;
            if (!w) throw sizeError('source width');
            if ((n / 4) % w) {
                throw domError('IndexSizeError', "Failed to construct 'ImageData': The input data length is not a multiple of (4 * width).");
            }
            h = n / 4 / w;
            if (c !== undefined && (c >>> 0) !== h) {
                throw domError('IndexSizeError', "Failed to construct 'ImageData': The input data length is not equal to (4 * width * height).");
            }
            settings(e, "construct 'ImageData'");
            bytes = a;
        } else {
            w = a >>> 0; h = b >>> 0;
            if (!w) throw sizeError('source width');
            if (!h) throw sizeError('source height');
            settings(c, "construct 'ImageData'");
            if (!(w * h * 4 <= MAX_BYTES)) throw domError('IndexSizeError', "Failed to construct 'ImageData': The requested image size exceeds the supported range.");
            bytes = zeros(w * h * 4, "construct 'ImageData'");
        }
        keepImg(this, w, h, bytes);
    };
    Object.defineProperty(Image.prototype, Symbol.toStringTag, { value: 'ImageData', configurable: true });
    Object.defineProperty(g, 'ImageData', { value: Image, writable: true, configurable: true, enumerable: false });
    var imageProto = Image.prototype;
    function imageData(w, h, bytes) { return keepImg(newRecord(imageProto), w, h, bytes); }
    function illegalImage() { throw new TypeErr('Illegal invocation'); }
    accessor(Image.prototype, 'width', function () { var v = IMG.get(this); return v ? v.width : illegalImage(); });
    accessor(Image.prototype, 'height', function () { var v = IMG.get(this); return v ? v.height : illegalImage(); });
    accessor(Image.prototype, 'data', function () { var v = IMG.get(this); return v ? v.data : illegalImage(); });
    accessor(Image.prototype, 'colorSpace', function () { return IMG.get(this) ? 'srgb' : illegalImage(); });
    method(P, 'getImageData', function getImageData(sx, sy, sw, sh, s) {
        need(4, arguments, 'CanvasRenderingContext2D', 'getImageData');
        var d = data(this);
        sx = +sx; sy = +sy; sw = +sw; sh = +sh;
        if (!finite(sx) || !finite(sy) || !finite(sw) || !finite(sh)) {
            throw new TypeErr("Failed to execute 'getImageData' on 'CanvasRenderingContext2D': The provided double value is non-finite.");
        }
        var w = trunc(sw), h = trunc(sh);
        if (w === 0) throw domError('IndexSizeError', "Failed to execute 'getImageData' on 'CanvasRenderingContext2D': The source width is 0.");
        if (h === 0) throw domError('IndexSizeError', "Failed to execute 'getImageData' on 'CanvasRenderingContext2D': The source height is 0.");
        settings(s, "execute 'getImageData' on 'CanvasRenderingContext2D'");
        var x = trunc(sx), y = trunc(sy);
        if (w < 0) { x += w; w = -w; }
        if (h < 0) { y += h; h = -h; }
        // The host leaves the bytes for `pixelsOut` and answers how many.
        var out = call(d, 'getImageData', x, y, w, h);
        if (out === 'SecurityError') throw domError('SecurityError', "Failed to execute 'getImageData' on 'CanvasRenderingContext2D': The canvas has been tainted by cross-origin data.");
        if (out !== w * h * 4) throw new RangeErr("Failed to execute 'getImageData' on 'CanvasRenderingContext2D': Out of memory at ImageData creation");
        return imageData(w, h, pixelsOut(out, false));
    });
    method(P, 'createImageData', function createImageData(a, b, s) {
        need(1, arguments, 'CanvasRenderingContext2D', 'createImageData');
        data(this);
        var v = imgOf(a);
        if (v) return imageData(v.width, v.height, zeros(v.width * v.height * 4, "execute 'createImageData' on 'CanvasRenderingContext2D'"));
        need(2, arguments, 'CanvasRenderingContext2D', 'createImageData');
        var w = absOf(trunc(+a)), h = absOf(trunc(+b));
        if (!w || !h) throw domError('IndexSizeError', "Failed to execute 'createImageData' on 'CanvasRenderingContext2D': The source " + (!w ? 'width' : 'height') + ' is 0.');
        settings(s, "execute 'createImageData' on 'CanvasRenderingContext2D'");
        return imageData(w, h, zeros(w * h * 4, "execute 'createImageData' on 'CanvasRenderingContext2D'"));
    });
    // putImageData(data, dx, dy[, dirtyX, dirtyY, dirtyWidth, dirtyHeight])
    // (HTML §4.12.5.1.15): the dirty rect normalised and cut to the image
    // here, the bytes read by the host from the array as it is now.
    method(P, 'putImageData', function putImageData(image, dx, dy, x, y, w, h) {
        need(3, arguments, 'CanvasRenderingContext2D', 'putImageData');
        var d = data(this), n = arguments.length;
        if (n > 3 && n < 7) throw new TypeErr("Failed to execute 'putImageData' on 'CanvasRenderingContext2D': Valid arities are: [3, 7], but " + n + ' arguments provided.');
        var v = imgOf(image);
        if (!v) throw new TypeErr("Failed to execute 'putImageData' on 'CanvasRenderingContext2D': parameter 1 is not of type 'ImageData'.");
        dx = +dx; dy = +dy;
        if (n < 7) { x = 0; y = 0; w = v.width; h = v.height; }
        else { x = +x; y = +y; w = +w; h = +h; }
        if (!finite(dx) || !finite(dy) || !finite(x) || !finite(y) || !finite(w) || !finite(h)) {
            throw new TypeErr("Failed to execute 'putImageData' on 'CanvasRenderingContext2D': The provided long value is non-finite.");
        }
        var held = pixelsIn(v.data, false);
        if (held !== v.width * v.height * 4) {
            throw domError('InvalidStateError', "Failed to execute 'putImageData' on 'CanvasRenderingContext2D': The source data has been detached.");
        }
        dx = trunc(dx); dy = trunc(dy); x = trunc(x); y = trunc(y); w = trunc(w); h = trunc(h);
        if (w < 0) { x += w; w = -w; }
        if (h < 0) { y += h; h = -h; }
        if (x < 0) { w += x; x = 0; }
        if (y < 0) { h += y; y = 0; }
        if (x + w > v.width) w = v.width - x;
        if (y + h > v.height) h = v.height - y;
        if (w <= 0 || h <= 0) return;
        pixelsIn(v.data, true);
        call(d, 'putImageData', v.width, v.height, dx, dy, x, y, w, h);
    });
    // An image source as [kind, NodeId, width, height]; null when it has
    // nothing to draw yet (an image still loading). What an `<img>` is as
    // a source is the engine's record of the element (`imageSource`: its
    // image's own size once it is in), not what its properties say.
    function source(img, member, op) {
        if (img instanceof Canvas) {
            var s = slot(img);
            if (!img.width || !img.height) {
                throw domError('InvalidStateError', "Failed to execute '" + member + "' on 'CanvasRenderingContext2D': The image argument is a canvas element with a width or height of 0.");
            }
            return ['canvas', s[1], img.width, img.height];
        }
        var at = null, answer;
        try { at = slot(img); } catch (e) { at = null; }
        answer = at ? C(at[0], at[1], op || 'imageSource') : 'TypeError';
        if (typeof answer !== 'string' || answer === 'TypeError') {
            throw new TypeError("Failed to execute '" + member + "' on 'CanvasRenderingContext2D': The provided value is not of type '(HTMLCanvasElement or HTMLImageElement)'.");
        }
        if (answer === '') return null;
        var parts = answer.split(',');
        return ['image', at[1], +parts[0], +parts[1], +parts[2]];
    }
    method(P, 'drawImage', function drawImage(img, a, b, c, e, f, h, i, j) {
        var n = arguments.length;
        if (n !== 3 && n !== 5 && n !== 9) {
            if (n < 3) need(3, arguments, 'CanvasRenderingContext2D', 'drawImage');
            throw new TypeError("Failed to execute 'drawImage' on 'CanvasRenderingContext2D': Valid arities are: [3, 5, 9], but " + n + ' arguments provided.');
        }
        var d = data(this), src = source(img, 'drawImage');
        if (!src) return;
        var r;
        if (n === 3) r = [0, 0, src[2], src[3], +a, +b, src[2], src[3]];
        else if (n === 5) r = [0, 0, src[2], src[3], +a, +b, +c, +e];
        else r = [+a, +b, +c, +e, +f, +h, +i, +j];
        if (!r.every(isFinite)) return;
        call(d, 'drawImage', src[0], src[1], r.join(','), src[2] + ',' + src[3]);
    });
    method(P, 'createPattern', function createPattern(img, repetition) {
        need(2, arguments, 'CanvasRenderingContext2D', 'createPattern');
        data(this);
        var rep = repetition === null || repetition === '' ? 'repeat' : String(repetition);
        if (['repeat', 'repeat-x', 'repeat-y', 'no-repeat'].indexOf(rep) < 0) {
            throw domError('SyntaxError', "Failed to execute 'createPattern' on 'CanvasRenderingContext2D': The provided type ('" + rep + "') is not one of 'repeat', 'no-repeat', 'repeat-x', or 'repeat-y'.");
        }
        var src = source(img, 'createPattern', 'imagePattern');
        if (!src) return null;
        var p = Object.create(Pattern.prototype);
        // A pattern keeps the image it was made from: the host's index of it.
        PAT.set(p, src[0] === 'image' ? { kind: 'pattern', src: src[4], repetition: rep } : { kind: src[0], src: src[1], repetition: rep });
        return p;
    });
})(typeof globalThis === 'object' ? globalThis : this);
