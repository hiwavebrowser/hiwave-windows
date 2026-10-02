// Standard interface objects that pages test with `instanceof` and `typeof`:
// the Event subclasses, the geometry types, interface-only objects for things
// the engine already hands to script (navigator, history, ...), and the
// prototypes of those existing objects. Evaluated after the Rust-backed DOM
// wrappers (dom.rs) exist. A name is only defined when nothing else defines
// it.
//
// What is deliberately NOT defined here: Worker, WebAssembly, Notification,
// AudioContext, OffscreenCanvas, CanvasRenderingContext2D, BroadcastChannel,
// MessageChannel, FileReader, Request/Response/Headers, ReadableStream and
// friends, Image/Option/Audio, CSS, CustomElementRegistry, Intl. Pages decide
// what to do by `typeof Worker`; a constructor that exists but does nothing
// would send them down the wrong path. Those arrive with real behaviour.
(function (g) {
    var Event = g.Event;
    if (typeof Event !== 'function') return;

    function def(name, value) {
        if (g[name] === undefined) {
            Object.defineProperty(g, name, { value: value, writable: true, configurable: true, enumerable: false });
        }
        return g[name];
    }
    function illegal() { throw new TypeError('Illegal constructor'); }
    // An interface object with no public constructor.
    function iface(name, parent) {
        if (g[name] !== undefined) return g[name];
        var ctor = function () { illegal(); };
        Object.defineProperty(ctor, 'name', { value: name });
        if (parent) Object.setPrototypeOf(ctor.prototype, parent.prototype);
        Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
        def(name, ctor);
        return ctor;
    }
    function coerce(kind, v) {
        return kind === 'bool' ? !!v : kind === 'num' ? Number(v) : kind === 'str' ? String(v) : v;
    }

    // ---- Event subclasses: name, parent, [field, default, kind]
    var K = ['ctrlKey', 'shiftKey', 'altKey', 'metaKey'];
    function mods() { return K.map(function (k) { return [k, false, 'bool']; }); }
    var EVENTS = [
        ['UIEvent', 'Event', [['view', null, 'any'], ['detail', 0, 'num']]],
        ['MouseEvent', 'UIEvent', [['screenX', 0, 'num'], ['screenY', 0, 'num'], ['clientX', 0, 'num'], ['clientY', 0, 'num'],
            ['button', 0, 'num'], ['buttons', 0, 'num'], ['relatedTarget', null, 'any']].concat(mods())],
        ['KeyboardEvent', 'UIEvent', [['key', '', 'str'], ['code', '', 'str'], ['location', 0, 'num'], ['repeat', false, 'bool'],
            ['isComposing', false, 'bool'], ['keyCode', 0, 'num'], ['charCode', 0, 'num']].concat(mods())],
        ['FocusEvent', 'UIEvent', [['relatedTarget', null, 'any']]],
        ['InputEvent', 'UIEvent', [['data', null, 'any'], ['isComposing', false, 'bool'], ['inputType', '', 'str']]],
        ['CompositionEvent', 'UIEvent', [['data', '', 'str']]],
        ['WheelEvent', 'MouseEvent', [['deltaX', 0, 'num'], ['deltaY', 0, 'num'], ['deltaZ', 0, 'num'], ['deltaMode', 0, 'num']]],
        ['PointerEvent', 'MouseEvent', [['pointerId', 0, 'num'], ['width', 1, 'num'], ['height', 1, 'num'], ['pressure', 0, 'num'],
            ['tiltX', 0, 'num'], ['tiltY', 0, 'num'], ['pointerType', '', 'str'], ['isPrimary', false, 'bool']]],
        ['DragEvent', 'MouseEvent', [['dataTransfer', null, 'any']]],
        ['TouchEvent', 'UIEvent', [['touches', [], 'any'], ['targetTouches', [], 'any'], ['changedTouches', [], 'any']].concat(mods())],
        ['MessageEvent', 'Event', [['data', null, 'any'], ['origin', '', 'str'], ['lastEventId', '', 'str'], ['source', null, 'any'], ['ports', [], 'any']]],
        ['ErrorEvent', 'Event', [['message', '', 'str'], ['filename', '', 'str'], ['lineno', 0, 'num'], ['colno', 0, 'num'], ['error', null, 'any']]],
        ['ProgressEvent', 'Event', [['lengthComputable', false, 'bool'], ['loaded', 0, 'num'], ['total', 0, 'num']]],
        ['PopStateEvent', 'Event', [['state', null, 'any']]],
        ['HashChangeEvent', 'Event', [['oldURL', '', 'str'], ['newURL', '', 'str']]],
        ['StorageEvent', 'Event', [['key', null, 'any'], ['oldValue', null, 'any'], ['newValue', null, 'any'], ['url', '', 'str'], ['storageArea', null, 'any']]],
        ['AnimationEvent', 'Event', [['animationName', '', 'str'], ['elapsedTime', 0, 'num'], ['pseudoElement', '', 'str']]],
        ['TransitionEvent', 'Event', [['propertyName', '', 'str'], ['elapsedTime', 0, 'num'], ['pseudoElement', '', 'str']]],
        ['ClipboardEvent', 'Event', [['clipboardData', null, 'any']]],
        ['PageTransitionEvent', 'Event', [['persisted', false, 'bool']]],
        ['BeforeUnloadEvent', 'Event', [['returnValue', '', 'str']]],
        ['MediaQueryListEvent', 'Event', [['media', '', 'str'], ['matches', false, 'bool']]]
    ];
    EVENTS.forEach(function (spec) {
        var name = spec[0], Parent = g[spec[1]], fields = spec[2];
        if (g[name] !== undefined || typeof Parent !== 'function') return;
        var C = function (type, init) {
            if (!(this instanceof C)) {
                throw new TypeError("Failed to construct '" + name + "': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
            }
            if (arguments.length < 1) {
                throw new TypeError("Failed to construct '" + name + "': 1 argument required, but only 0 present.");
            }
            Parent.call(this, type, init);
            var i = init || {};
            for (var n = 0; n < fields.length; n++) {
                var f = fields[n], v = i[f[0]];
                this[f[0]] = v === undefined ? (Array.isArray(f[1]) ? f[1].slice() : f[1]) : coerce(f[2], v);
            }
        };
        Object.defineProperty(C, 'name', { value: name });
        C.prototype = Object.create(Parent.prototype, { constructor: { value: C, writable: true, configurable: true } });
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
        def(name, C);
    });
    var ME = g.MouseEvent;
    if (ME && ME.prototype.getModifierState === undefined) {
        ME.prototype.getModifierState = function (key) {
            return { Control: this.ctrlKey, Shift: this.shiftKey, Alt: this.altKey, Meta: this.metaKey }[key] === true;
        };
        ['x', 'pageX', 'offsetX'].forEach(function (k) {
            Object.defineProperty(ME.prototype, k, { get: function () { return this.clientX; }, configurable: true, enumerable: true });
        });
        ['y', 'pageY', 'offsetY'].forEach(function (k) {
            Object.defineProperty(ME.prototype, k, { get: function () { return this.clientY; }, configurable: true, enumerable: true });
        });
    }
    var KE = g.KeyboardEvent;
    if (KE) {
        ['DOM_KEY_LOCATION_STANDARD', 'DOM_KEY_LOCATION_LEFT', 'DOM_KEY_LOCATION_RIGHT', 'DOM_KEY_LOCATION_NUMPAD'].forEach(function (k, i) {
            KE[k] = KE.prototype[k] = i;
        });
        if (KE.prototype.getModifierState === undefined) KE.prototype.getModifierState = ME ? ME.prototype.getModifierState : function () { return false; };
    }

    // ---- Geometry (Geometry Interfaces §4-§5)
    function num(v, d) { return v === undefined ? d : Number(v); }
    if (g.DOMPointReadOnly === undefined) {
        var DOMPointReadOnly = function DOMPointReadOnly(x, y, z, w) {
            if (!(this instanceof DOMPointReadOnly)) throw new TypeError("Failed to construct 'DOMPointReadOnly': Please use the 'new' operator.");
            this.x = num(x, 0); this.y = num(y, 0); this.z = num(z, 0); this.w = num(w, 1);
        };
        DOMPointReadOnly.fromPoint = function (p) { p = p || {}; return new DOMPointReadOnly(p.x, p.y, p.z, p.w); };
        DOMPointReadOnly.prototype.toJSON = function () { return { x: this.x, y: this.y, z: this.z, w: this.w }; };
        def('DOMPointReadOnly', DOMPointReadOnly);
        var DOMPoint = function DOMPoint(x, y, z, w) {
            if (!(this instanceof DOMPoint)) throw new TypeError("Failed to construct 'DOMPoint': Please use the 'new' operator.");
            DOMPointReadOnly.call(this, x, y, z, w);
        };
        DOMPoint.prototype = Object.create(DOMPointReadOnly.prototype, { constructor: { value: DOMPoint, writable: true, configurable: true } });
        DOMPoint.fromPoint = function (p) { p = p || {}; return new DOMPoint(p.x, p.y, p.z, p.w); };
        def('DOMPoint', DOMPoint);
    }
    if (g.DOMRectReadOnly === undefined) {
        var DOMRectReadOnly = function DOMRectReadOnly(x, y, width, height) {
            if (!(this instanceof DOMRectReadOnly)) throw new TypeError("Failed to construct 'DOMRectReadOnly': Please use the 'new' operator.");
            this.x = num(x, 0); this.y = num(y, 0); this.width = num(width, 0); this.height = num(height, 0);
        };
        var edges = {
            top: function () { return Math.min(this.y, this.y + this.height); },
            bottom: function () { return Math.max(this.y, this.y + this.height); },
            left: function () { return Math.min(this.x, this.x + this.width); },
            right: function () { return Math.max(this.x, this.x + this.width); }
        };
        Object.keys(edges).forEach(function (k) {
            Object.defineProperty(DOMRectReadOnly.prototype, k, { get: edges[k], configurable: true, enumerable: true });
        });
        DOMRectReadOnly.fromRect = function (r) { r = r || {}; return new DOMRectReadOnly(r.x, r.y, r.width, r.height); };
        DOMRectReadOnly.prototype.toJSON = function () {
            return { x: this.x, y: this.y, width: this.width, height: this.height,
                     top: this.top, right: this.right, bottom: this.bottom, left: this.left };
        };
        def('DOMRectReadOnly', DOMRectReadOnly);
        var DOMRect = function DOMRect(x, y, width, height) {
            if (!(this instanceof DOMRect)) throw new TypeError("Failed to construct 'DOMRect': Please use the 'new' operator.");
            DOMRectReadOnly.call(this, x, y, width, height);
        };
        DOMRect.prototype = Object.create(DOMRectReadOnly.prototype, { constructor: { value: DOMRect, writable: true, configurable: true } });
        DOMRect.fromRect = function (r) { r = r || {}; return new DOMRect(r.x, r.y, r.width, r.height); };
        def('DOMRect', DOMRect);
    }

    // ---- Interface objects for things the engine already hands to script.
    var EventTarget = g.EventTarget, Node = g.Node;
    var Window = g.Window;
    if (Window === undefined) {
        Window = function Window() { illegal(); };
        if (EventTarget) Object.setPrototypeOf(Window.prototype, EventTarget.prototype);
        // `window` is the global object, not an instance of a constructor we
        // could build, so `instanceof Window` is answered by identity.
        Object.defineProperty(Window, Symbol.hasInstance, { value: function (v) { return v === g; } });
        def('Window', Window);
    }
    function tag(obj, Iface) {
        if (obj && typeof obj === 'object' && Iface) {
            try { Object.setPrototypeOf(obj, Iface.prototype); } catch (e) {}
        }
    }
    tag(g.navigator, iface('Navigator'));
    tag(g.history, iface('History'));
    tag(g.location, iface('Location'));
    tag(g.screen, iface('Screen', EventTarget));
    tag(g.performance, iface('Performance', EventTarget));
    var Storage = iface('Storage');
    tag(g.localStorage, Storage);
    tag(g.sessionStorage, Storage);
    var MQL = iface('MediaQueryList', EventTarget);
    if (typeof g.matchMedia === 'function' && !g.matchMedia.__rkTagged) {
        var mm = g.matchMedia;
        var tagged = function matchMedia() {
            var r = mm.apply(this, arguments);
            tag(r, MQL);
            if (r && r.addListener === undefined) {
                r.addListener = function (cb) { if (typeof r.addEventListener === 'function') r.addEventListener('change', cb); };
                r.removeListener = function (cb) { if (typeof r.removeEventListener === 'function') r.removeEventListener('change', cb); };
            }
            return r;
        };
        tagged.__rkTagged = true;
        g.matchMedia = tagged;
    }
    // Interface-only: no instance is produced yet, but `x instanceof Attr`
    // and `typeof ShadowRoot` guards must have a right-hand side.
    iface('ShadowRoot', g.DocumentFragment || Node);
    iface('Attr', Node);
    iface('NamedNodeMap');
    iface('DOMStringMap');
    var StyleSheet = iface('StyleSheet');
    iface('CSSStyleSheet', StyleSheet);
    iface('CSSRule');
    iface('Range');
    iface('Selection');
    iface('TreeWalker');
    iface('NodeIterator');
    iface('DOMImplementation');
    iface('FileList');
    iface('DataTransfer');
    iface('MutationRecord');
    iface('ResizeObserverEntry');
    iface('IntersectionObserverEntry');
    iface('PerformanceEntry');
    iface('Plugin');
    iface('MimeType');
})(globalThis);
