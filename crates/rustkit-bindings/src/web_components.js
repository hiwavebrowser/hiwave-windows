// Custom Elements (HTML §4.13): customElements.define / get / getName /
// whenDefined / upgrade, a constructible HTMLElement, and the lifecycle
// reactions connectedCallback, disconnectedCallback and
// attributeChangedCallback.
//
// Pure JS over the DOM bindings: it wraps the tree-mutation and attribute
// methods of the node wrappers and does nothing (one length check) until
// a page defines its first element. Stated limits: autonomous custom
// elements only (`define(name, ctor, { extends })` throws NotSupportedError,
// as in Safari); adoptedCallback never fires (one document); `:defined` is not
// a selector; ElementInternals / form-associated elements are not provided.
// Shadow DOM is not here (see the shadow root work).
(function (g) {
    if (g.customElements) return;
    var document = g.document;
    if (!document || !g.HTMLElement) return;

    var OldHTMLElement = g.HTMLElement;
    var names = [];                 // defined names, in definition order
    var byName = {};                // name -> definition
    var byCtor = typeof Map === 'function' ? new Map() : null;
    var pending = {};               // name -> [resolve] for whenDefined
    var state = typeof WeakMap === 'function' ? new WeakMap() : new Map();
    var stack = [];                 // upgrade/construct in progress
    var rawCreateElement = null;    // the DOM's own createElement, below
    var selector = '';              // joined defined names, for subtree scans

    function domError(message, name) {
        return typeof g.DOMException === 'function' ? new g.DOMException(message, name) : (function () { var e = new Error(message); e.name = name; return e; })();
    }
    function report(e) {
        try { if (g.__rustkit_errors) g.__rustkit_errors.push(String(e)); } catch (_) {}
    }
    function call(fn, thisArg, args) {
        try { fn.apply(thisArg, args); } catch (e) { report(e); }
    }

    // PotentialCustomElementName: a lowercase letter, a hyphen somewhere
    // after it, no uppercase, and not one of the reserved names.
    var RESERVED = ['annotation-xml', 'color-profile', 'font-face', 'font-face-src', 'font-face-uri',
                    'font-face-format', 'font-face-name', 'missing-glyph'];
    function validName(name) {
        return /^[a-z][-._0-9a-z·À-ÖØ-öø-ͽͿ-῿‌‍‿⁀⁰-↏Ⰰ-⿯、-퟿豈-﷏ﷰ-�]*-[-._0-9a-z·À-ÖØ-öø-ͽͿ-῿‌‍‿⁀⁰-↏Ⰰ-⿯、-퟿豈-﷏ﷰ-�]*$/.test(name) &&
            RESERVED.indexOf(name) < 0;
    }

    function stateOf(el) { return state.get(el); }

    // ---- reactions
    function connectedNow(el) { return !!el.isConnected; }
    function enqueueConnected(el, st) {
        if (st.connected || !connectedNow(el)) return;
        st.connected = true;
        if (st.def.connected) call(st.def.connected, el, []);
    }
    function enqueueDisconnected(el, st) {
        if (!st.connected || connectedNow(el)) return;
        st.connected = false;
        if (st.def.disconnected) call(st.def.disconnected, el, []);
    }

    // Upgrade: run the definition's constructor on an element that already
    // exists (HTML "upgrade an element").
    function upgrade(el, def) {
        var existing = stateOf(el);
        if (existing && (existing.def === def || existing.failed)) return;
        var record = { element: el, constructed: false };
        var st = { def: def, connected: false, failed: false };
        Object.setPrototypeOf(el, def.ctor.prototype);
        state.set(el, st);
        stack.push(record);
        var result, threw = false;
        try {
            result = new def.ctor();
        } catch (e) {
            threw = true;
            report(e);
        } finally {
            stack.pop();
        }
        if (threw || result !== el) {
            if (!threw) report(domError('The custom element constructor did not produce the element being upgraded.', 'InvalidStateError'));
            st.failed = true;
            return;
        }
        // Attributes present at upgrade time are "changed" from null.
        var observed = def.observed;
        if (observed.length && def.attributeChanged && typeof el.getAttributeNames === 'function') {
            el.getAttributeNames().forEach(function (n) {
                if (observed.indexOf(n) >= 0) call(def.attributeChanged, el, [n, null, el.getAttribute(n)]);
            });
        }
        enqueueConnected(el, st);
    }

    // Everything in `root` (and root itself) whose name is defined.
    function candidates(root) {
        if (!selector || !root) return [];
        var out = [];
        try {
            if (root.nodeType === 1 && root.matches(selector)) out.push(root);
            if (typeof root.querySelectorAll === 'function') {
                var found = root.querySelectorAll(selector);
                for (var i = 0; i < found.length; i++) out.push(found[i]);
            }
        } catch (e) { report(e); }
        return out;
    }
    function definitionOf(el) {
        if (el.nodeType !== 1) return null;
        return byName[String(el.localName).toLowerCase()] || null;
    }
    // After an insertion: upgrade what can be, and connect what is now connected.
    function reconcile(root) {
        candidates(root).forEach(function (el) {
            var def = definitionOf(el);
            if (!def) return;
            var st = stateOf(el);
            if (!st) { upgrade(el, def); return; }
            if (!st.failed) enqueueConnected(el, st);
        });
    }
    // Before a removal: the connected custom elements that may leave the document.
    function connectedCustom(root) {
        return candidates(root).filter(function (el) {
            var st = stateOf(el);
            return st && !st.failed && st.connected;
        });
    }
    function afterRemoval(list) {
        list.forEach(function (el) { var st = stateOf(el); if (st) enqueueDisconnected(el, st); });
    }

    // ---- the constructible HTMLElement
    function HTMLElement() {
        var target = new.target;
        if (!target) throw new TypeError("Failed to construct 'HTMLElement': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        var def = byCtor ? byCtor.get(target) : null;
        if (!def) throw new TypeError('Illegal constructor');
        var top = stack[stack.length - 1];
        if (top && !top.constructed) {
            // Under an upgrade: `super()` hands back the element being upgraded.
            top.constructed = true;
            return top.element;
        }
        // `new MyElement()`: a fresh element with the defined name.
        var el = rawCreateElement.call(document, def.name);
        Object.setPrototypeOf(el, target.prototype);
        state.set(el, { def: def, connected: false, failed: false });
        return el;
    }
    HTMLElement.prototype = OldHTMLElement.prototype;
    Object.defineProperty(HTMLElement.prototype, 'constructor', { value: HTMLElement, writable: true, configurable: true });
    Object.defineProperty(HTMLElement, 'name', { value: 'HTMLElement' });
    Object.setPrototypeOf(HTMLElement, Object.getPrototypeOf(OldHTMLElement));
    Object.defineProperty(g, 'HTMLElement', { value: HTMLElement, writable: true, configurable: true, enumerable: false });

    // document.createElement builds a defined element synchronously.
    var DocumentProto = Object.getPrototypeOf(document);
    var originalCreate = DocumentProto.createElement;
    rawCreateElement = originalCreate;
    DocumentProto.createElement = function (tag) {
        var el = originalCreate.apply(this, arguments);
        if (!names.length) return el;
        var def = definitionOf(el);
        if (def) {
            try {
                var made = new def.ctor();
                return made;
            } catch (e) {
                report(e);
                return el; // an element that failed to construct stays a plain one
            }
        }
        return el;
    };

    // ---- the registry
    function CustomElementRegistry() { throw new TypeError('Illegal constructor'); }
    CustomElementRegistry.prototype.define = function (name, ctor, options) {
        name = String(name);
        if (typeof ctor !== 'function') throw new TypeError("Failed to execute 'define' on 'CustomElementRegistry': The callback provided as parameter 2 is not a constructor.");
        if (!validName(name)) throw domError("Failed to execute 'define' on 'CustomElementRegistry': \"" + name + '" is not a valid custom element name', 'SyntaxError');
        if (byName[name]) throw domError("Failed to execute 'define' on 'CustomElementRegistry': the name \"" + name + '" has already been used with this registry', 'NotSupportedError');
        if (byCtor && byCtor.has(ctor)) throw domError("Failed to execute 'define' on 'CustomElementRegistry': this constructor has already been used with this registry", 'NotSupportedError');
        if (options && options.extends !== undefined && options.extends !== null) {
            throw domError("Failed to execute 'define' on 'CustomElementRegistry': customized built-in elements are not supported", 'NotSupportedError');
        }
        var proto = ctor.prototype;
        if (proto === null || (typeof proto !== 'object' && typeof proto !== 'function')) {
            throw new TypeError("Failed to execute 'define' on 'CustomElementRegistry': The constructor's 'prototype' is not an object.");
        }
        function callback(key) {
            var fn = proto[key];
            if (fn !== undefined && fn !== null && typeof fn !== 'function') {
                throw new TypeError("Failed to execute 'define' on 'CustomElementRegistry': The '" + key + "' property is not callable.");
            }
            return fn || null;
        }
        var def = {
            name: name, ctor: ctor,
            connected: callback('connectedCallback'), disconnected: callback('disconnectedCallback'),
            attributeChanged: callback('attributeChangedCallback'), observed: []
        };
        if (def.attributeChanged) {
            var list = ctor.observedAttributes;
            if (list !== undefined && list !== null) def.observed = Array.from(list).map(String);
        }
        byName[name] = def;
        if (byCtor) byCtor.set(ctor, def);
        names.push(name);
        selector = names.join(',');
        // Every connected element of that name that already exists upgrades.
        var found = document.getElementsByTagName(name);
        Array.prototype.slice.call(found).forEach(function (el) { upgrade(el, def); });
        if (pending[name]) {
            pending[name].forEach(function (resolve) { resolve(ctor); });
            delete pending[name];
        }
    };
    CustomElementRegistry.prototype.get = function (name) {
        var def = byName[String(name)];
        return def ? def.ctor : undefined;
    };
    CustomElementRegistry.prototype.getName = function (ctor) {
        var def = byCtor ? byCtor.get(ctor) : null;
        return def ? def.name : null;
    };
    CustomElementRegistry.prototype.whenDefined = function (name) {
        name = String(name);
        if (!validName(name)) {
            return Promise.reject(domError("Failed to execute 'whenDefined' on 'CustomElementRegistry': \"" + name + '" is not a valid custom element name', 'SyntaxError'));
        }
        if (byName[name]) return Promise.resolve(byName[name].ctor);
        return new Promise(function (resolve) { (pending[name] || (pending[name] = [])).push(resolve); });
    };
    CustomElementRegistry.prototype.upgrade = function (root) {
        reconcile(root);
    };
    Object.defineProperty(CustomElementRegistry.prototype, Symbol.toStringTag, { value: 'CustomElementRegistry', configurable: true });
    var registry = Object.create(CustomElementRegistry.prototype);
    Object.defineProperty(g, 'CustomElementRegistry', { value: CustomElementRegistry, writable: true, configurable: true, enumerable: false });
    Object.defineProperty(g, 'customElements', { value: registry, writable: true, configurable: true, enumerable: true });

    // ---- wrapping the tree and attribute mutators
    function protoWith(name) {
        return [g.Node, g.Element, g.Document, g.DocumentFragment, g.CharacterData].filter(function (C) {
            return C && C.prototype && Object.prototype.hasOwnProperty.call(C.prototype, name);
        }).map(function (C) { return C.prototype; });
    }
    function nodesIn(args) {
        var out = [];
        Array.prototype.forEach.call(args, function (a) {
            if (a && typeof a === 'object' && a.nodeType) {
                if (a.nodeType === 11) Array.prototype.forEach.call(a.childNodes, function (c) { out.push(c); });
                else out.push(a);
            }
        });
        return out;
    }
    // Insertions: reconcile what was inserted. `which` names the argument
    // positions that are nodes (all, or the first).
    function wrapInsert(name, firstOnly) {
        protoWith(name).forEach(function (proto) {
            var native = proto[name];
            if (typeof native !== 'function' || native.__rkCE) return;
            var wrapped = function () {
                if (!names.length) return native.apply(this, arguments);
                var inserted = nodesIn(firstOnly ? [arguments[0]] : arguments);
                // A node that is already connected and is inserted again is a
                // move: disconnected, then connected at its new place.
                var moving = [];
                inserted.forEach(function (n) { moving = moving.concat(connectedCustom(n)); });
                // A replacement also removes: collect what leaves first.
                var leaving = [];
                if (name === 'replaceChild' && arguments[1]) leaving = connectedCustom(arguments[1]);
                if (name === 'replaceWith') leaving = connectedCustom(this);
                if (name === 'replaceChildren') { var self = this; leaving = connectedCustom(this).filter(function (el) { return el !== self; }); }
                var result = native.apply(this, arguments);
                afterRemoval(leaving);
                moving.forEach(function (el) {
                    var st = stateOf(el);
                    if (!st || !st.connected) return;
                    st.connected = false;
                    if (st.def.disconnected) call(st.def.disconnected, el, []);
                });
                inserted.forEach(reconcile);
                return result;
            };
            wrapped.__rkCE = true;
            Object.defineProperty(proto, name, { value: wrapped, writable: true, configurable: true, enumerable: Object.prototype.propertyIsEnumerable.call(proto, name) });
        });
    }
    ['appendChild', 'insertBefore', 'replaceChild'].forEach(function (n) { wrapInsert(n, true); });
    ['append', 'prepend', 'before', 'after', 'replaceWith', 'replaceChildren'].forEach(function (n) { wrapInsert(n, false); });

    function wrapRemove(name, victim) {
        protoWith(name).forEach(function (proto) {
            var native = proto[name];
            if (typeof native !== 'function' || native.__rkCE) return;
            var wrapped = function () {
                if (!names.length) return native.apply(this, arguments);
                var leaving = connectedCustom(victim(this, arguments));
                var result = native.apply(this, arguments);
                afterRemoval(leaving);
                return result;
            };
            wrapped.__rkCE = true;
            Object.defineProperty(proto, name, { value: wrapped, writable: true, configurable: true, enumerable: Object.prototype.propertyIsEnumerable.call(proto, name) });
        });
    }
    wrapRemove('removeChild', function (self, args) { return args[0]; });
    wrapRemove('remove', function (self) { return self; });

    // innerHTML / textContent setters replace the children: those leave.
    function wrapSetter(proto, prop, replacesChildren) {
        var d = Object.getOwnPropertyDescriptor(proto, prop);
        if (!d || !d.set || d.set.__rkCE) return;
        var set = d.set;
        var wrapped = function (v) {
            if (!names.length) return set.call(this, v);
            var leaving = connectedCustom(this).filter(function (el) { return el !== this; }, this);
            var result = set.call(this, v);
            afterRemoval(leaving);
            if (prop === 'innerHTML') reconcile(this);
            return result;
        };
        wrapped.__rkCE = true;
        Object.defineProperty(proto, prop, { get: d.get, set: wrapped, enumerable: d.enumerable, configurable: true });
    }
    wrapSetter(g.Element.prototype, 'innerHTML');
    wrapSetter(g.Node.prototype, 'textContent');

    protoWith('insertAdjacentHTML').forEach(function (proto) {
        var native = proto.insertAdjacentHTML;
        if (native.__rkCE) return;
        var wrapped = function (where) {
            var result = native.apply(this, arguments);
            if (names.length) {
                var w = String(where).toLowerCase();
                reconcile(w === 'beforebegin' || w === 'afterend' ? this.parentNode : this);
            }
            return result;
        };
        wrapped.__rkCE = true;
        Object.defineProperty(proto, 'insertAdjacentHTML', { value: wrapped, writable: true, configurable: true, enumerable: Object.prototype.propertyIsEnumerable.call(proto, 'insertAdjacentHTML') });
    });

    // Attributes: attributeChangedCallback for the observed ones.
    function wrapAttribute(name, newValue) {
        var native = g.Element.prototype[name];
        if (typeof native !== 'function' || native.__rkCE) return;
        var wrapped = function (attr) {
            if (!names.length) return native.apply(this, arguments);
            var st = stateOf(this);
            var key = String(attr);
            var watched = st && !st.failed && st.def.attributeChanged && st.def.observed.indexOf(key) >= 0;
            var old = watched ? this.getAttribute(key) : null;
            var result = native.apply(this, arguments);
            if (watched) {
                var now = this.getAttribute(key);
                if (name !== 'removeAttribute' || old !== null) call(st.def.attributeChanged, this, [key, old, now]);
            }
            return result;
        };
        wrapped.__rkCE = true;
        Object.defineProperty(g.Element.prototype, name, { value: wrapped, writable: true, configurable: true, enumerable: Object.prototype.propertyIsEnumerable.call(g.Element.prototype, name) });
    }
    wrapAttribute('setAttribute');
    wrapAttribute('removeAttribute');
})(typeof globalThis === 'object' ? globalThis : this);
