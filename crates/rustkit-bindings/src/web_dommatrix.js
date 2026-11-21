// DOMMatrix and DOMMatrixReadOnly (Geometry Interfaces §4): the 4x4 matrix
// pages build from a computed `transform` (`new DOMMatrix(getComputedStyle(el).transform)`)
// to read translation and scale, and animation code composes with. Read by
// carousels and galleries (apple.com's gallery constructs one at load).
//
// Parsing takes `none`, `matrix()`, `matrix3d()`, `translate[XYZ|3d]()`,
// `scale[XYZ|3d]()`, `rotate[XYZ]()`, `skew[XY]()` (px, deg, rad, turn, unitless).
// Not here: `perspective()` and `rotate3d()` in strings, `toFloat64Array`.
(function (g) {
    if (typeof g.DOMMatrix === 'function') return;

    var IDENTITY = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
    var NAMES = ['m11', 'm12', 'm13', 'm14', 'm21', 'm22', 'm23', 'm24', 'm31', 'm32', 'm33', 'm34', 'm41', 'm42', 'm43', 'm44'];
    var V = Symbol('rustkit.matrix');   // the 16 numbers, column-major: m11 m12 m13 m14 m21 ...
    var TWO_D = Symbol('rustkit.is2D');

    function mul(a, b) {            // a x b: b is applied first
        var r = new Array(16);
        for (var c = 0; c < 4; c++) {
            for (var row = 0; row < 4; row++) {
                var s = 0;
                for (var k = 0; k < 4; k++) s += a[k * 4 + row] * b[c * 4 + k];
                r[c * 4 + row] = s;
            }
        }
        return r;
    }
    function translation(x, y, z) { var m = IDENTITY.slice(); m[12] = x; m[13] = y; m[14] = z; return m; }
    function scaling(x, y, z) { var m = IDENTITY.slice(); m[0] = x; m[5] = y; m[10] = z; return m; }
    function rad(deg) { return deg * Math.PI / 180; }
    function rotZ(deg) { var c = Math.cos(rad(deg)), s = Math.sin(rad(deg)); var m = IDENTITY.slice(); m[0] = c; m[1] = s; m[4] = -s; m[5] = c; return m; }
    function rotY(deg) { var c = Math.cos(rad(deg)), s = Math.sin(rad(deg)); var m = IDENTITY.slice(); m[0] = c; m[2] = -s; m[8] = s; m[10] = c; return m; }
    function rotX(deg) { var c = Math.cos(rad(deg)), s = Math.sin(rad(deg)); var m = IDENTITY.slice(); m[5] = c; m[6] = s; m[9] = -s; m[10] = c; return m; }
    function skew(xDeg, yDeg) { var m = IDENTITY.slice(); m[4] = Math.tan(rad(xDeg)); m[1] = Math.tan(rad(yDeg)); return m; }

    function inverse(m) {
        var inv = new Array(16);
        inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
        inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
        inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
        inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
        inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
        inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
        inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
        inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
        inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
        inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
        inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
        inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
        inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
        inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
        inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
        inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
        var det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
        if (det === 0 || !isFinite(det)) return null;
        for (var i = 0; i < 16; i++) inv[i] = inv[i] / det;
        return inv;
    }

    function num(s) { return parseFloat(s); }
    function angle(s) {
        s = s.trim();
        var n = parseFloat(s);
        if (/rad$/.test(s)) return n * 180 / Math.PI;
        if (/turn$/.test(s)) return n * 360;
        if (/grad$/.test(s)) return n * 0.9;
        return n;   // deg, or unitless
    }
    function parse(text) {
        text = String(text).trim();
        if (text === '' || text === 'none') return { m: IDENTITY.slice(), is2D: true };
        var m = IDENTITY.slice(), is2D = true, re = /([a-zA-Z0-9]+)\(([^)]*)\)/g, hit, any = false;
        while ((hit = re.exec(text))) {
            any = true;
            var name = hit[1], a = hit[2].split(',').map(function (x) { return x.trim(); }), t = null;
            var n = a.map(num);
            switch (name) {
                case 'matrix': t = [n[0], n[1], 0, 0, n[2], n[3], 0, 0, 0, 0, 1, 0, n[4], n[5], 0, 1]; break;
                case 'matrix3d': t = n.slice(0, 16); is2D = false; break;
                case 'translate': t = translation(n[0] || 0, n[1] || 0, 0); break;
                case 'translateX': t = translation(n[0] || 0, 0, 0); break;
                case 'translateY': t = translation(0, n[0] || 0, 0); break;
                case 'translateZ': t = translation(0, 0, n[0] || 0); is2D = false; break;
                case 'translate3d': t = translation(n[0] || 0, n[1] || 0, n[2] || 0); is2D = false; break;
                case 'scale': t = scaling(n[0], a.length > 1 ? n[1] : n[0], 1); break;
                case 'scaleX': t = scaling(n[0], 1, 1); break;
                case 'scaleY': t = scaling(1, n[0], 1); break;
                case 'scaleZ': t = scaling(1, 1, n[0]); is2D = false; break;
                case 'scale3d': t = scaling(n[0], n[1], n[2]); is2D = false; break;
                case 'rotate': case 'rotateZ': t = rotZ(angle(a[0])); if (name === 'rotateZ') is2D = false; break;
                case 'rotateX': t = rotX(angle(a[0])); is2D = false; break;
                case 'rotateY': t = rotY(angle(a[0])); is2D = false; break;
                case 'skew': t = skew(angle(a[0]), a.length > 1 ? angle(a[1]) : 0); break;
                case 'skewX': t = skew(angle(a[0]), 0); break;
                case 'skewY': t = skew(0, angle(a[0])); break;
                default: throw new SyntaxError("Failed to parse '" + text + "': unsupported transform function '" + name + "'.");
            }
            for (var i = 0; i < 16; i++) if (typeof t[i] !== 'number' || isNaN(t[i])) throw new SyntaxError("Failed to parse '" + text + "'.");
            m = mul(m, t);
        }
        if (!any) throw new SyntaxError("Failed to parse '" + text + "'.");
        return { m: m, is2D: is2D };
    }

    function init(self, args) {
        var a = args[0], v, is2D;
        if (a === undefined) { v = IDENTITY.slice(); is2D = true; }
        else if (typeof a === 'string') { var p = parse(a); v = p.m; is2D = p.is2D; }
        else if (a !== null && typeof a === 'object' && typeof a.length === 'number') {
            if (a.length === 6) { v = [a[0], a[1], 0, 0, a[2], a[3], 0, 0, 0, 0, 1, 0, a[4], a[5], 0, 1]; is2D = true; }
            else if (a.length === 16) { v = Array.prototype.slice.call(a); is2D = false; }
            else throw new TypeError("Failed to construct 'DOMMatrix': The sequence must contain 6 elements for a 2D matrix or 16 elements for a 3D matrix.");
        } else throw new TypeError("Failed to construct 'DOMMatrix': The provided value is not of type '(DOMString or sequence<unrestricted double>)'.");
        self[V] = v.map(Number);
        self[TWO_D] = is2D;
    }

    function DOMMatrixReadOnly() {
        if (!(this instanceof DOMMatrixReadOnly)) throw new TypeError("Failed to construct 'DOMMatrixReadOnly': Please use the 'new' operator.");
        init(this, arguments);
    }
    var RO = DOMMatrixReadOnly.prototype;
    function matrixOf(self) {
        var v = self && self[V];
        if (!v) throw new TypeError('Illegal invocation');
        return v;
    }
    function make(Ctor, v, is2D) { var o = Object.create(Ctor.prototype); o[V] = v; o[TWO_D] = is2D; return o; }
    NAMES.forEach(function (name, i) {
        Object.defineProperty(RO, name, { get: function () { return matrixOf(this)[i]; }, configurable: true, enumerable: true });
    });
    [['a', 0], ['b', 1], ['c', 4], ['d', 5], ['e', 12], ['f', 13]].forEach(function (p) {
        Object.defineProperty(RO, p[0], { get: function () { return matrixOf(this)[p[1]]; }, configurable: true, enumerable: true });
    });
    Object.defineProperty(RO, 'is2D', { get: function () { matrixOf(this); return this[TWO_D]; }, configurable: true, enumerable: true });
    Object.defineProperty(RO, 'isIdentity', {
        get: function () { var v = matrixOf(this); return v.every(function (x, i) { return x === IDENTITY[i]; }); },
        configurable: true, enumerable: true
    });
    function pm(name, fn) { Object.defineProperty(RO, name, { value: fn, writable: true, configurable: true, enumerable: true }); }
    function other(o) {
        if (o instanceof DOMMatrixReadOnly) return o[V];
        if (o !== null && typeof o === 'object') return DOMMatrixReadOnly.fromMatrix.call(DOMMatrix, o)[V];
        throw new TypeError("Failed to execute 'multiply' on 'DOMMatrixReadOnly': The provided value is not of type 'DOMMatrixInit'.");
    }
    pm('multiply', function multiply(o) { var v = matrixOf(this); var w = other(o === undefined ? new DOMMatrix() : o); return make(DOMMatrix, mul(v, w), this[TWO_D] && (o ? o[TWO_D] !== false : true)); });
    pm('translate', function translate(x, y, z) { var v = matrixOf(this); z = z || 0; return make(DOMMatrix, mul(v, translation(+x || 0, +y || 0, +z)), this[TWO_D] && !z); });
    pm('scale', function scale(sx, sy, sz, ox, oy, oz) {
        var v = matrixOf(this);
        sx = sx === undefined ? 1 : +sx; sy = sy === undefined ? sx : +sy; sz = sz === undefined ? 1 : +sz;
        ox = +ox || 0; oy = +oy || 0; oz = +oz || 0;
        var t = mul(mul(translation(ox, oy, oz), scaling(sx, sy, sz)), translation(-ox, -oy, -oz));
        return make(DOMMatrix, mul(v, t), this[TWO_D] && sz === 1 && !oz);
    });
    pm('scaleNonUniform', function scaleNonUniform(sx, sy) { return this.scale(sx, sy === undefined ? 1 : sy, 1); });
    pm('scale3d', function scale3d(s, ox, oy, oz) { return this.scale(s, s, s, ox, oy, oz); });
    pm('rotate', function rotate(rx, ry, rz) {
        var v = matrixOf(this);
        if (ry === undefined && rz === undefined) { rz = rx; rx = 0; ry = 0; }
        rx = +rx || 0; ry = +ry || 0; rz = +rz || 0;
        var t = mul(mul(rotZ(rz), rotY(ry)), rotX(rx));
        return make(DOMMatrix, mul(v, t), this[TWO_D] && !rx && !ry);
    });
    pm('rotateAxisAngle', function rotateAxisAngle(x, y, z, angleDeg) {
        var v = matrixOf(this);
        x = +x || 0; y = +y || 0; z = +z || 0;
        var len = Math.sqrt(x * x + y * y + z * z);
        if (!len) return make(DOMMatrix, v.slice(), this[TWO_D]);
        x /= len; y /= len; z /= len;
        var a = rad(+angleDeg || 0), c = Math.cos(a), s = Math.sin(a), t = 1 - c;
        var r = [t * x * x + c, t * x * y + s * z, t * x * z - s * y, 0,
                 t * x * y - s * z, t * y * y + c, t * y * z + s * x, 0,
                 t * x * z + s * y, t * y * z - s * x, t * z * z + c, 0,
                 0, 0, 0, 1];
        return make(DOMMatrix, mul(v, r), this[TWO_D] && x === 0 && y === 0);
    });
    pm('skewX', function skewX(deg) { return make(DOMMatrix, mul(matrixOf(this), skew(+deg || 0, 0)), this[TWO_D]); });
    pm('skewY', function skewY(deg) { return make(DOMMatrix, mul(matrixOf(this), skew(0, +deg || 0)), this[TWO_D]); });
    pm('flipX', function flipX() { return make(DOMMatrix, mul(matrixOf(this), scaling(-1, 1, 1)), this[TWO_D]); });
    pm('flipY', function flipY() { return make(DOMMatrix, mul(matrixOf(this), scaling(1, -1, 1)), this[TWO_D]); });
    pm('inverse', function inverseOf() {
        var inv = inverse(matrixOf(this));
        if (!inv) return make(DOMMatrix, new Array(16).fill(NaN), this[TWO_D]);
        return make(DOMMatrix, inv, this[TWO_D]);
    });
    pm('transformPoint', function transformPoint(p) {
        var v = matrixOf(this);
        p = p || {};
        var x = +p.x || 0, y = +p.y || 0, z = +p.z || 0, w = p.w === undefined ? 1 : +p.w;
        var X = v[0] * x + v[4] * y + v[8] * z + v[12] * w;
        var Y = v[1] * x + v[5] * y + v[9] * z + v[13] * w;
        var Z = v[2] * x + v[6] * y + v[10] * z + v[14] * w;
        var W = v[3] * x + v[7] * y + v[11] * z + v[15] * w;
        return typeof g.DOMPoint === 'function' ? new g.DOMPoint(X, Y, Z, W) : { x: X, y: Y, z: Z, w: W };
    });
    pm('toFloat32Array', function toFloat32Array() { return new Float32Array(matrixOf(this)); });
    pm('toFloat64Array', function toFloat64Array() { return new Float64Array(matrixOf(this)); });
    pm('toJSON', function toJSON() {
        var out = {};
        ['a', 'b', 'c', 'd', 'e', 'f'].concat(NAMES).forEach(function (k) { out[k] = this[k]; }, this);
        out.is2D = this.is2D; out.isIdentity = this.isIdentity;
        return out;
    });
    pm('toString', function toString() {
        var v = matrixOf(this);
        if (v.some(function (x) { return !isFinite(x); })) {
            throw new g.DOMException("Failed to execute 'toString' on 'DOMMatrixReadOnly': Cannot be serialized with NaN or Infinity values.", 'InvalidStateError');
        }
        var fmt = function (x) { return String(Math.abs(x) < 1e-6 ? 0 : x); };
        if (this[TWO_D]) return 'matrix(' + [v[0], v[1], v[4], v[5], v[12], v[13]].map(fmt).join(', ') + ')';
        return 'matrix3d(' + v.map(fmt).join(', ') + ')';
    });
    DOMMatrixReadOnly.fromMatrix = function fromMatrix(o) {
        o = o || {};
        var v = IDENTITY.slice();
        var is2D = true;
        var map = { a: 0, b: 1, c: 4, d: 5, e: 12, f: 13 };
        Object.keys(map).forEach(function (k) { if (o[k] !== undefined) v[map[k]] = +o[k]; });
        NAMES.forEach(function (k, i) { if (o[k] !== undefined) { v[i] = +o[k]; if (!(i === 0 || i === 1 || i === 4 || i === 5 || i === 12 || i === 13) && v[i] !== IDENTITY[i]) is2D = false; } });
        return make(this === DOMMatrixReadOnly ? DOMMatrixReadOnly : DOMMatrix, v, o.is2D === undefined ? is2D : !!o.is2D);
    };
    Object.defineProperty(RO, Symbol.toStringTag, { value: 'DOMMatrixReadOnly', configurable: true });

    function DOMMatrix() {
        if (!(this instanceof DOMMatrix)) throw new TypeError("Failed to construct 'DOMMatrix': Please use the 'new' operator.");
        init(this, arguments);
    }
    DOMMatrix.prototype = Object.create(RO, { constructor: { value: DOMMatrix, writable: true, configurable: true } });
    Object.setPrototypeOf(DOMMatrix, DOMMatrixReadOnly);
    var W = DOMMatrix.prototype;
    // Writable members: the m## and a..f attributes.
    NAMES.forEach(function (name, i) {
        Object.defineProperty(W, name, {
            get: function () { return matrixOf(this)[i]; },
            set: function (x) {
                matrixOf(this)[i] = +x;
                if (!(i === 0 || i === 1 || i === 4 || i === 5 || i === 12 || i === 13) && +x !== IDENTITY[i]) this[TWO_D] = false;
            },
            configurable: true, enumerable: true
        });
    });
    [['a', 0], ['b', 1], ['c', 4], ['d', 5], ['e', 12], ['f', 13]].forEach(function (p) {
        Object.defineProperty(W, p[0], { get: function () { return matrixOf(this)[p[1]]; }, set: function (x) { matrixOf(this)[p[1]] = +x; }, configurable: true, enumerable: true });
    });
    function self(name, fn) {
        Object.defineProperty(W, name, {
            value: function () {
                var r = fn.apply(this, arguments);
                this[V] = r[V]; this[TWO_D] = r[TWO_D];
                return this;
            }, writable: true, configurable: true, enumerable: true
        });
    }
    self('multiplySelf', function (o) { return RO.multiply.call(this, o); });
    self('preMultiplySelf', function (o) { return make(DOMMatrix, mul(other(o), matrixOf(this)), this[TWO_D]); });
    self('translateSelf', function (x, y, z) { return RO.translate.call(this, x, y, z); });
    self('scaleSelf', function (a, b, c, d, e, f) { return RO.scale.call(this, a, b, c, d, e, f); });
    self('scale3dSelf', function (s, ox, oy, oz) { return RO.scale3d.call(this, s, ox, oy, oz); });
    self('rotateSelf', function (a, b, c) { return RO.rotate.call(this, a, b, c); });
    self('rotateAxisAngleSelf', function (x, y, z, a) { return RO.rotateAxisAngle.call(this, x, y, z, a); });
    self('skewXSelf', function (d) { return RO.skewX.call(this, d); });
    self('skewYSelf', function (d) { return RO.skewY.call(this, d); });
    self('invertSelf', function () { return RO.inverse.call(this); });
    Object.defineProperty(W, 'setMatrixValue', {
        value: function setMatrixValue(text) { var p = parse(text); this[V] = p.m; this[TWO_D] = p.is2D; return this; },
        writable: true, configurable: true, enumerable: true
    });
    DOMMatrix.fromMatrix = function fromMatrix(o) { return DOMMatrixReadOnly.fromMatrix.call(DOMMatrix, o); };
    DOMMatrix.fromFloat32Array = function (a) { return new DOMMatrix(a); };
    DOMMatrix.fromFloat64Array = function (a) { return new DOMMatrix(a); };
    Object.defineProperty(W, Symbol.toStringTag, { value: 'DOMMatrix', configurable: true });

    ['DOMMatrixReadOnly', 'DOMMatrix', 'WebKitCSSMatrix'].forEach(function (name) {
        Object.defineProperty(g, name, { value: name === 'DOMMatrixReadOnly' ? DOMMatrixReadOnly : DOMMatrix, writable: true, configurable: true, enumerable: false });
    });
})(typeof globalThis === 'object' ? globalThis : this);
