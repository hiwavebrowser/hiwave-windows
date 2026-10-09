// Node APIs over the wrapper layer (see node_apis.rs): Element.attributes
// and Attr, document.activeElement with focus()/blur(), createElementNS and
// the *AttributeNS methods, and document.createEvent.
(function (g) {
    var I = g.__rkNodeInternals, NS = __rustkit_dom_ns;
    delete g.__rkNodeInternals;
    delete g.__rustkit_dom_ns;
    var Document = g.Document, Element = g.Element, DOMException = g.DOMException;
    var HTML_NS = 'http://www.w3.org/1999/xhtml', SVG_NS = 'http://www.w3.org/2000/svg';
    // The namespaces a stored "prefix:local" attribute name can stand for;
    // rustkit-dom keeps attributes by qualified name only.
    var PREFIX_NS = { xml: 'http://www.w3.org/XML/1998/namespace',
                      xmlns: 'http://www.w3.org/2000/xmlns/', xlink: 'http://www.w3.org/1999/xlink' };

    function illegal() { throw new TypeError('Illegal constructor'); }
    function iface(name, parent) {
        var ctor = function () { illegal(); };
        Object.defineProperty(ctor, 'name', { value: name });
        if (parent) Object.setPrototypeOf(ctor.prototype, parent.prototype);
        Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: name });
        Object.defineProperty(g, name, { value: ctor, writable: true, configurable: true });
        return ctor;
    }
    function getter(proto, name, fn) {
        Object.defineProperty(proto, name, { get: fn, configurable: true, enumerable: true });
    }
    function fail(method, on, name) {
        throw new DOMException("Failed to execute '" + method + "' on '" + on + "'.", name);
    }

    // ---- Namespaces (DOM §4.9): an element's namespaceURI is the one it
    // was created in; parsed and createElement elements are HTML.
    getter(Element.prototype, 'namespaceURI', function () {
        var s = I.slotOf(this); return NS(s.gen, 'of', s.id);
    });
    // SVG elements get the SVG interfaces (the tag table already maps
    // `svg`); elements in any other non-HTML namespace are plain Element.
    I.extendElementProto(function (id, base) {
        var ns = NS(I.gen(), 'of', id);
        if (ns === null || ns === HTML_NS) return base(id);
        if (ns !== SVG_NS) return Element.prototype;
        var p = base(id);
        return p === g.SVGSVGElement.prototype ? p : g.SVGElement.prototype;
    });
    Document.prototype.createElementNS = function (ns, qualified) {
        var r = NS(I.gen(), 'create', ns == null ? '' : String(ns), String(qualified));
        if (typeof r === 'string') fail('createElementNS', 'Document', r);
        return I.wrap(r);
    };
    function localOf(name) { var i = name.indexOf(':'); return i < 0 ? name : name.slice(i + 1); }
    function nsOf(name) { var i = name.indexOf(':'); return i < 0 ? null : PREFIX_NS[name.slice(0, i)] || null; }
    // The stored name for (namespace, localName), or null.
    function nsName(el, ns, local) {
        ns = ns == null || ns === '' ? null : String(ns);
        local = String(local);
        var names = el.getAttributeNames();
        for (var i = 0; i < names.length; i++) {
            var n = names[i];
            if (ns === null ? n === local : localOf(n) === local && nsOf(n) === ns) return n;
            if (ns === PREFIX_NS.xmlns && n === 'xmlns' && local === 'xmlns') return n;
        }
        return null;
    }
    Element.prototype.setAttributeNS = function (ns, qualified, value) {
        var r = NS(I.gen(), 'check', ns == null ? '' : String(ns), String(qualified));
        if (typeof r === 'string') fail('setAttributeNS', 'Element', r);
        this.setAttribute(qualified, value);
    };
    Element.prototype.getAttributeNS = function (ns, local) {
        var n = nsName(this, ns, local); return n === null ? null : this.getAttribute(n);
    };
    Element.prototype.hasAttributeNS = function (ns, local) { return nsName(this, ns, local) !== null; };
    Element.prototype.removeAttributeNS = function (ns, local) {
        var n = nsName(this, ns, local); if (n !== null) this.removeAttribute(n);
    };

    // ---- Element.attributes (DOM §4.9.1): a live NamedNodeMap of Attr
    // nodes. Every read asks the element, so it never goes stale; the Attr
    // for a name stays the same object. Order is getAttributeNames' order.
    var NamedNodeMap = iface('NamedNodeMap'), Attr = iface('Attr', g.Node);
    var OWNER = Symbol('rustkit.owner'), NAME = Symbol('rustkit.name');
    var maps = new WeakMap(), attrs = new WeakMap();
    function hidden(o, k, v) { Object.defineProperty(o, k, { value: v }); return o; }
    function attrNode(el, name) {
        var byName = attrs.get(el);
        if (!byName) attrs.set(el, byName = Object.create(null));
        return byName[name] || (byName[name] = hidden(hidden(Object.create(Attr.prototype), OWNER, el), NAME, name));
    }
    // The stored name `name` refers to on `el`, or null (HTML lowercases).
    function storedName(el, name) {
        name = String(name);
        if (el.namespaceURI === HTML_NS) name = name.toLowerCase();
        return el.getAttributeNames().indexOf(name) >= 0 ? name : null;
    }
    getter(Attr.prototype, 'name', function () { return this[NAME]; });
    getter(Attr.prototype, 'nodeName', function () { return this[NAME]; });
    getter(Attr.prototype, 'localName', function () { return localOf(this[NAME]); });
    getter(Attr.prototype, 'prefix', function () {
        var n = this[NAME], i = n.indexOf(':'); return i < 0 ? null : n.slice(0, i);
    });
    getter(Attr.prototype, 'namespaceURI', function () {
        return this[NAME] === 'xmlns' ? PREFIX_NS.xmlns : nsOf(this[NAME]);
    });
    getter(Attr.prototype, 'ownerElement', function () {
        return this[OWNER].hasAttribute(this[NAME]) ? this[OWNER] : null;
    });
    getter(Attr.prototype, 'nodeType', function () { return 2; });
    getter(Attr.prototype, 'specified', function () { return true; });
    getter(Attr.prototype, 'parentNode', function () { return null; });
    getter(Attr.prototype, 'ownerDocument', function () { return this[OWNER].ownerDocument; });
    ['value', 'nodeValue', 'textContent'].forEach(function (k) {
        Object.defineProperty(Attr.prototype, k, {
            get: function () { var v = this[OWNER].getAttribute(this[NAME]); return v === null ? '' : v; },
            set: function (v) { this[OWNER].setAttribute(this[NAME], v); },
            configurable: true, enumerable: true
        });
    });

    getter(NamedNodeMap.prototype, 'length', function () { return this[OWNER].getAttributeNames().length; });
    NamedNodeMap.prototype.item = function (i) {
        var n = this[OWNER].getAttributeNames()[i >>> 0];
        return n === undefined ? null : attrNode(this[OWNER], n);
    };
    NamedNodeMap.prototype.getNamedItem = function (name) {
        var n = storedName(this[OWNER], name); return n === null ? null : attrNode(this[OWNER], n);
    };
    NamedNodeMap.prototype.getNamedItemNS = function (ns, local) {
        var n = nsName(this[OWNER], ns, local); return n === null ? null : attrNode(this[OWNER], n);
    };
    NamedNodeMap.prototype.removeNamedItem = function (name) {
        var a = this.getNamedItem(name);
        if (!a) fail('removeNamedItem', 'NamedNodeMap', 'NotFoundError');
        this[OWNER].removeAttribute(a.name);
        return a;
    };
    NamedNodeMap.prototype[Symbol.iterator] = function () {
        var out = [];
        for (var i = 0; i < this.length; i++) out.push(this.item(i));
        return out[Symbol.iterator]();
    };
    // `attributes[i]` is live too, so the map is a Proxy over its indices.
    var INDEX = /^(0|[1-9][0-9]*)$/;
    function isIndex(t, k) { return typeof k === 'string' && INDEX.test(k) && Number(k) < t.length; }
    var traps = {
        get: function (t, k, r) { return isIndex(t, k) ? t.item(Number(k)) : Reflect.get(t, k, r); },
        has: function (t, k) { return isIndex(t, k) || Reflect.has(t, k); },
        ownKeys: function (t) {
            var keys = [];
            for (var i = 0; i < t.length; i++) keys.push(String(i));
            return keys.concat(Reflect.ownKeys(t));
        },
        getOwnPropertyDescriptor: function (t, k) {
            return isIndex(t, k) ? { value: t.item(Number(k)), enumerable: true, configurable: true }
                                 : Reflect.getOwnPropertyDescriptor(t, k);
        }
    };
    getter(Element.prototype, 'attributes', function () {
        var m = maps.get(this);
        if (!m) maps.set(this, m = new Proxy(hidden(Object.create(NamedNodeMap.prototype), OWNER, this), traps));
        return m;
    });
    Element.prototype.getAttributeNode = function (name) { return this.attributes.getNamedItem(name); };
    Element.prototype.getAttributeNodeNS = function (ns, local) {
        return this.attributes.getNamedItemNS(ns, local);
    };

    // ---- document.activeElement with focus()/blur() (HTML §6.6). One
    // focus for script and the user: the engine moves it on a click
    // (`__rkSetFocus`) and follows what script did (`__rkTakeFocus`).
    // Nothing focused, or the focused element gone from the document,
    // answers the body.
    var focused = null, moved = false;
    function inDocument(el) {
        var n = el;
        while (n.parentNode) n = n.parentNode;
        return n === g.document;
    }
    // HTML §6.6.2 "focusable area", for the cases a page reaches by script.
    function isFocusable(el) {
        if (!inDocument(el)) return false;
        var tag = el.localName;
        if (/^(input|select|textarea|button)$/.test(tag) && el.hasAttribute('disabled')) return false;
        if (tag === 'input') return (el.getAttribute('type') || '').toLowerCase() !== 'hidden';
        if (/^(select|textarea|button|iframe|summary)$/.test(tag)) return true;
        if ((tag === 'a' || tag === 'area') && el.hasAttribute('href')) return true;
        var editable = el.getAttribute('contenteditable');
        if (editable !== null && /^(|true|plaintext-only)$/i.test(editable)) return true;
        return el.hasAttribute('tabindex') && !isNaN(parseInt(el.getAttribute('tabindex'), 10));
    }
    function current() { return focused && inDocument(focused) ? focused : null; }
    function fire(el, type, related) {
        var FE = g.FocusEvent || g.Event;
        el.dispatchEvent(new FE(type, { bubbles: type === 'focusin' || type === 'focusout',
                                        composed: true, relatedTarget: related }));
    }
    // A field the user edited commits when it loses the focus: `change`
    // (HTML §4.10.5.5). `__rkFireInput` marks the edit.
    function commit(el) {
        if (!el.__rkEdited) return;
        el.__rkEdited = false;
        el.dispatchEvent(new g.Event('change', { bubbles: true }));
    }
    // The focus update steps: change, then blur/focusout on the old
    // element (with nothing focused), then focus/focusin on the new one.
    function focus() {
        var old = current();
        if (old === this || !isFocusable(this)) return;
        focused = null;
        moved = true;
        if (old) { commit(old); fire(old, 'blur', this); fire(old, 'focusout', this); }
        focused = this;
        fire(this, 'focus', old);
        fire(this, 'focusin', old);
    }
    function blur() {
        if (current() !== this) return;
        focused = null;
        moved = true;
        commit(this);
        fire(this, 'blur', null);
        fire(this, 'focusout', null);
    }
    // The user's click focused an element (by node id), or landed on
    // nothing focusable (null): the same steps script's focus() runs.
    Object.defineProperty(Document.prototype, '__rkSetFocus', {
        value: function (id) {
            var el = typeof id === 'number' ? I.wrap(id) : null;
            var old = current();
            if (el) focus.call(el);
            if (old && current() === old && old !== el) blur.call(old);
            // The engine always asks afterwards: the page may have refused.
            moved = true;
        },
        configurable: true, enumerable: false, writable: true
    });
    // Where the focus is, when it moved since the engine last asked: the
    // node id, -1 for nothing; undefined when it has not moved.
    Object.defineProperty(Document.prototype, '__rkTakeFocus', {
        value: function () {
            if (!moved) return undefined;
            moved = false;
            var el = current(), s = el && I.slotOf(el);
            return s && s.gen === I.gen() ? s.id : -1;
        },
        configurable: true, enumerable: false, writable: true
    });
    [g.HTMLElement, g.SVGElement].forEach(function (C) {
        C.prototype.focus = focus;
        C.prototype.blur = blur;
    });
    getter(Document.prototype, 'activeElement', function () {
        var el = this === g.document ? current() : null;
        return el || this.body || this.documentElement;
    });

    // ---- document.createEvent (DOM §4.5): an event of the named interface
    // with an empty type, which dispatchEvent refuses until initEvent (or
    // initCustomEvent) has run.
    var UNINIT = Symbol('rustkit.uninitialised');
    var EVENT_INTERFACES = {
        beforeunloadevent: 'BeforeUnloadEvent', compositionevent: 'CompositionEvent',
        customevent: 'CustomEvent', dragevent: 'DragEvent', event: 'Event', events: 'Event',
        focusevent: 'FocusEvent', hashchangeevent: 'HashChangeEvent', htmlevents: 'Event',
        keyboardevent: 'KeyboardEvent', messageevent: 'MessageEvent', mouseevent: 'MouseEvent',
        mouseevents: 'MouseEvent', storageevent: 'StorageEvent', svgevents: 'Event',
        touchevent: 'TouchEvent', uievent: 'UIEvent', uievents: 'UIEvent'
    };
    Document.prototype.createEvent = function (name) {
        var C = g[EVENT_INTERFACES[String(name).toLowerCase()]];
        if (typeof C !== 'function') {
            throw new DOMException("Failed to execute 'createEvent' on 'Document': The provided event type ('" +
                name + "') is invalid.", 'NotSupportedError');
        }
        var e = new C('');
        e[UNINIT] = true;
        return e;
    };
    var initEvent = g.Event.prototype.initEvent;
    g.Event.prototype.initEvent = function () {
        if (this.eventPhase === 0) delete this[UNINIT];
        return initEvent.apply(this, arguments);
    };
    var dispatch = g.EventTarget.prototype.dispatchEvent;
    function dispatchEvent(event) {
        if (event != null && event[UNINIT]) {
            throw new DOMException("Failed to execute 'dispatchEvent' on 'EventTarget': " +
                'The event provided is uninitialized.', 'InvalidStateError');
        }
        return dispatch.apply(this, arguments);
    }
    g.EventTarget.prototype.dispatchEvent = dispatchEvent;
    // The global took its own copy of the method (dom.rs).
    if (g.dispatchEvent === dispatch) g.dispatchEvent = dispatchEvent;
})(globalThis);
