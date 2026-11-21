// Standard interface objects that pages test with `instanceof` and `typeof`:
// the Event subclasses, the geometry types, interface-only objects for things
// the engine already hands to script (navigator, history, ...), and the
// prototypes of those existing objects. Evaluated after the Rust-backed DOM
// wrappers (dom.rs) exist. A name is only defined when nothing else defines
// it.
//
// What is deliberately NOT defined here: WebAssembly, Notification,
// AudioContext, OffscreenCanvas, BroadcastChannel,
// MessageChannel, Request/Response/Headers, ReadableStream and
// friends, Image/Option/Audio, CSS, CustomElementRegistry, Intl. Pages decide
// what to do by `typeof WebAssembly`; a constructor that exists but does nothing
// would send them down the wrong path. Those arrive with real behaviour
// (CanvasRenderingContext2D has: web_canvas.js).
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
            ['button', 0, 'num'], ['buttons', 0, 'num'], ['relatedTarget', null, 'any'],
            ['movementX', 0, 'num'], ['movementY', 0, 'num']].concat(mods())],
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
        ['MediaQueryListEvent', 'Event', [['media', '', 'str'], ['matches', false, 'bool']]],
        ['PromiseRejectionEvent', 'Event', [['promise', null, 'any'], ['reason', undefined, 'any']]],
        ['SubmitEvent', 'Event', [['submitter', null, 'any']]]
    ];
    function fieldsOf(name) {
        for (var n = 0; n < EVENTS.length; n++) if (EVENTS[n][0] === name) return EVENTS[n][2];
        return [];
    }
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
        // The engine's own reference, on the host object: it builds its
        // events with these whatever a page puts on the window since.
        if (name === 'FocusEvent') __rk.FocusEvent = C;
        C.prototype = Object.create(Parent.prototype, { constructor: { value: C, writable: true, configurable: true } });
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
        __rk.keepCtor(name, C, spec[1], fields);
        def(name, C);
    });
    var ME = g.MouseEvent;
    if (ME && ME.prototype.getModifierState === undefined) {
        ME.prototype.getModifierState = function (key) {
            return { Control: this.ctrlKey, Shift: this.shiftKey, Alt: this.altKey, Meta: this.metaKey }[key] === true;
        };
        ['x', 'pageX'].forEach(function (k) {
            Object.defineProperty(ME.prototype, k, { get: function () { return this.clientX; }, configurable: true, enumerable: true });
        });
        ['y', 'pageY'].forEach(function (k) {
            Object.defineProperty(ME.prototype, k, { get: function () { return this.clientY; }, configurable: true, enumerable: true });
        });
        // offsetX / offsetY (CSSOM View §9): from the padding edge of the
        // event's target, worked out when read. For a target inside an
        // `<svg>` that is the outermost `<svg>`'s (Chrome for Testing 148);
        // an event that has no target yet answers its client point. The
        // engine's own events carry the numbers and do not come here.
        function offsetFrom(e, axis) {
            var client = axis === 'x' ? e.clientX : e.clientY, t = e.target;
            if (!t || t.nodeType !== 1 || typeof t.getBoundingClientRect !== 'function') return client;
            if (t.namespaceURI === 'http://www.w3.org/2000/svg') {
                for (var n = t; n && n.nodeType === 1 && n.namespaceURI === t.namespaceURI; n = n.parentNode) {
                    if (n.localName === 'svg') t = n;
                }
            }
            var r = t.getBoundingClientRect();
            return axis === 'x' ? client - r.left - (t.clientLeft || 0) : client - r.top - (t.clientTop || 0);
        }
        // layerX / layerY (legacy): from the border edge of the nearest
        // positioned element at or above the target, in page coordinates;
        // the page point when there is none.
        function layerFrom(e, axis) {
            var page = axis === 'x' ? e.pageX : e.pageY;
            for (var n = e.target; n && n.nodeType === 1; n = n.parentNode) {
                var position = typeof g.getComputedStyle === 'function' ? g.getComputedStyle(n).position : '';
                if (position && position !== 'static') {
                    var r = n.getBoundingClientRect();
                    return axis === 'x' ? page - (r.left + (g.scrollX || 0)) : page - (r.top + (g.scrollY || 0));
                }
            }
            return page;
        }
        [['offsetX', offsetFrom, 'x'], ['offsetY', offsetFrom, 'y'], ['layerX', layerFrom, 'x'], ['layerY', layerFrom, 'y']].forEach(function (spec) {
            Object.defineProperty(ME.prototype, spec[0], {
                get: function () { return spec[1](this, spec[2]); }, configurable: true, enumerable: true
            });
        });
    }
    // WheelEvent's legacy deltas and its mode constants (UI Events §5.5;
    // measured in Chrome for Testing 148, ten inits in the SVG2 fixture):
    // `wheelDeltaX` / `wheelDeltaY` are what the init gave, else the
    // event's own deltaX / deltaY with the SAME sign, as whole numbers
    // (100.7 reads 100, 0.5 reads 0); `wheelDelta` is the Y one, else the X
    // one, and a `wheelDelta` in the init is not read.
    var WE = g.WheelEvent;
    if (WE && !Object.prototype.hasOwnProperty.call(WE.prototype, 'wheelDelta')) {
        var WHEEL = typeof WeakMap === 'function' ? new WeakMap() : null;
        var WheelEventWithLegacy = function WheelEvent(type, init) {
            var e = Reflect.construct(WE, arguments.length < 2 ? [type] : [type, init], new.target || WheelEventWithLegacy);
            var i = init || {};
            if (WHEEL) {
                var whole = function (v) { v = Math.trunc(Number(v)); return isFinite(v) ? v : 0; };
                WHEEL.set(e, {
                    x: whole(i.wheelDeltaX === undefined ? e.deltaX : i.wheelDeltaX),
                    y: whole(i.wheelDeltaY === undefined ? e.deltaY : i.wheelDeltaY)
                });
            }
            return e;
        };
        WheelEventWithLegacy.prototype = WE.prototype;
        Object.defineProperty(WE.prototype, 'constructor', { value: WheelEventWithLegacy, writable: true, configurable: true });
        Object.setPrototypeOf(WheelEventWithLegacy, Object.getPrototypeOf(WE));
        var legacy = function (e) { return (WHEEL && WHEEL.get(e)) || { x: Math.trunc(e.deltaX) || 0, y: Math.trunc(e.deltaY) || 0 }; };
        Object.defineProperty(WE.prototype, 'wheelDeltaX', { get: function () { return legacy(this).x; }, configurable: true, enumerable: true });
        Object.defineProperty(WE.prototype, 'wheelDeltaY', { get: function () { return legacy(this).y; }, configurable: true, enumerable: true });
        Object.defineProperty(WE.prototype, 'wheelDelta', {
            get: function () { var l = legacy(this); return l.y || l.x; }, configurable: true, enumerable: true
        });
        [['DOM_DELTA_PIXEL', 0], ['DOM_DELTA_LINE', 1], ['DOM_DELTA_PAGE', 2]].forEach(function (pair) {
            [WheelEventWithLegacy, WE.prototype].forEach(function (o) {
                Object.defineProperty(o, pair[0], { value: pair[1], enumerable: true });
            });
        });
        g.WheelEvent = WheelEventWithLegacy;
        __rk.keepCtor('WheelEvent', WheelEventWithLegacy, 'MouseEvent', fieldsOf('WheelEvent'));
        // The engine's wheel events get their legacy deltas here, with
        // built-ins taken at install.
        var wheelSet = Function.prototype.call.bind(WeakMap.prototype.set), trunc = Math.trunc, finite = isFinite;
        var hasOwnInit = Function.prototype.call.bind(Object.prototype.hasOwnProperty);
        __rk.keepFinish('WheelEvent', function (e, i) {
            if (!WHEEL) return;
            var x = hasOwnInit(i, 'wheelDeltaX') ? i.wheelDeltaX : e.deltaX;
            var y = hasOwnInit(i, 'wheelDeltaY') ? i.wheelDeltaY : e.deltaY;
            x = trunc(x); y = trunc(y);
            wheelSet(WHEEL, e, { x: finite(x) ? x : 0, y: finite(y) ? y : 0 });
        });
    }
    // UIEvent.which (UI Events §7.3, legacy): a key event's keyCode, a mouse
    // event's button + 1, else 0. Chrome for Testing 148 has it on
    // UIEvent.prototype alone, and ignores a `which` in the init dictionary:
    // `new KeyboardEvent('keyup', { keyCode: 13 }).which` is 13.
    if (g.UIEvent && !Object.prototype.hasOwnProperty.call(g.UIEvent.prototype, 'which')) {
        Object.defineProperty(g.UIEvent.prototype, 'which', {
            // By what the event carries, not by `instanceof`: an event made
            // with another window's constructor is still a key event here.
            get: function () {
                if (typeof this.keyCode === 'number' && typeof this.charCode === 'number') return this.keyCode;
                if (typeof this.button === 'number') return this.button + 1;
                return 0;
            },
            configurable: true, enumerable: true
        });
    }
    // TextEvent (UI Events §7.2, legacy): what `textInput` is. Script cannot
    // construct one; `document.createEvent('TextEvent')` and the engine's
    // own `textInput` (dom.rs `fireTextInput`) make them here, through a
    // maker on the host object, which no page script holds.
    if (g.UIEvent && g.TextEvent === undefined) {
        var makingText = false;
        var TextEvent = function TextEvent(type, init) {
            if (!makingText) illegal();
            g.UIEvent.call(this, type, init);
            this.data = init && init.data !== undefined ? String(init.data) : '';
        };
        TextEvent.prototype = Object.create(g.UIEvent.prototype, { constructor: { value: TextEvent, writable: true, configurable: true } });
        Object.defineProperty(TextEvent.prototype, Symbol.toStringTag, { value: 'TextEvent', configurable: true });
        TextEvent.prototype.initTextEvent = function initTextEvent(type, bubbles, cancelable, view, data) {
            if (__rk.isDispatching(this)) return;
            this.initEvent(type, !!bubbles, !!cancelable);
            this.view = view || null;
            this.data = data === undefined ? 'undefined' : String(data);
        };
        def('TextEvent', TextEvent);
        __rk.keepCtor('TextEvent', TextEvent, 'UIEvent', [['data', '']]);
        __rk.newTextEvent = function (type, init) {
            makingText = true;
            try { return new TextEvent(type, init); } finally { makingText = false; }
        };
    }
    // InputEvent.getTargetRanges() (Input Events §4.1.1): the ranges the
    // edit will affect, as StaticRanges. A new array of new ranges on every
    // call, as in Chrome; empty unless the event was made with
    // `targetRanges`.
    if (g.InputEvent && g.InputEvent.prototype.getTargetRanges === undefined) {
        var IE = g.InputEvent, TARGETS = typeof WeakMap === 'function' ? new WeakMap() : null;
        var InputEventWithRanges = function InputEvent(type, init) {
            var e = Reflect.construct(IE, arguments.length < 2 ? [type] : [type, init], new.target || InputEventWithRanges);
            var ranges = init && init.targetRanges;
            if (ranges != null && TARGETS) {
                TARGETS.set(e, Array.from(ranges, function (r) {
                    if (!(g.StaticRange && r instanceof g.StaticRange)) {
                        throw new TypeError("Failed to construct 'InputEvent': Failed to read the 'targetRanges' property from 'InputEventInit': Failed to convert value to 'StaticRange'.");
                    }
                    return r;
                }));
            }
            return e;
        };
        InputEventWithRanges.prototype = IE.prototype;
        Object.defineProperty(IE.prototype, 'constructor', { value: InputEventWithRanges, writable: true, configurable: true });
        Object.setPrototypeOf(InputEventWithRanges, Object.getPrototypeOf(IE));
        IE.prototype.getTargetRanges = function getTargetRanges() {
            if (!(this instanceof IE)) throw new TypeError('Illegal invocation');
            var ranges = TARGETS && TARGETS.get(this);
            return (ranges || []).map(function (r) {
                return new g.StaticRange({ startContainer: r.startContainer, startOffset: r.startOffset,
                                           endContainer: r.endContainer, endOffset: r.endOffset });
            });
        };
        g.InputEvent = InputEventWithRanges;
        __rk.keepCtor('InputEvent', InputEventWithRanges, 'UIEvent', fieldsOf('InputEvent'));
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
        Object.defineProperty(DOMRectReadOnly.prototype, Symbol.toStringTag, { value: 'DOMRectReadOnly', configurable: true });
        Object.defineProperty(DOMRect.prototype, Symbol.toStringTag, { value: 'DOMRect', configurable: true });
        DOMRect.fromRect = function (r) { r = r || {}; return new DOMRect(r.x, r.y, r.width, r.height); };
        def('DOMRect', DOMRect);
    }

    // ---- Interface objects for things the engine already hands to script.
    var EventTarget = g.EventTarget, Node = g.Node;
    var Window = g.Window;
    if (Window === undefined) {
        Window = function Window() { illegal(); };
        if (EventTarget) Object.setPrototypeOf(Window.prototype, EventTarget.prototype);
        Object.setPrototypeOf(g, Window.prototype);
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
    if (typeof g.matchMedia === 'function' && !__rk.matchMediaTagged) {
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
        __rk.matchMediaTagged = true;
        g.matchMedia = tagged;
    }
    // Interface-only: no instance is produced yet, but `x instanceof Attr`
    // and `typeof ShadowRoot` guards must have a right-hand side.
    iface('ShadowRoot', g.DocumentFragment || Node);
    iface('Attr', Node);
    iface('CDATASection', g.Text || g.CharacterData || Node);
    var PI = iface('ProcessingInstruction', g.CharacterData || Node);
    if (PI && PI.prototype) {
        Object.defineProperty(PI.prototype, 'target', { get: function () { return ''; }, configurable: true, enumerable: true });
    }
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
