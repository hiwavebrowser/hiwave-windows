// Shadow DOM, the script-visible half (DOM §4.8, HTML §4.2.2): attachShadow,
// Element.shadowRoot, ShadowRoot (a DocumentFragment with host/mode/
// innerHTML), <slot> assignment (assignedNodes/assignedElements,
// assignedSlot), and the event path through a shadow boundary (composed
// events go on to the host; a listener outside sees the host as the target;
// composedPath()).
//
// The shadow tree is part of the one Rust DOM (rustkit-dom `shadow.rs`): a
// shadow root is a node its host owns, and slot assignment is computed
// there. Script reads and writes it through `__rkShadowInternals`
// (dom/shadow.rs). Style, layout and paint walk the flat tree built from
// the same nodes, so what is attached here renders: a host shows its shadow
// tree, a slot its assigned nodes (or its fallback), and light children
// assigned to no slot do not render. Declarative shadow roots
// (`<template shadowrootmode>`) are attached by the parser.
//
// A <style> in a shadow root styles that tree only, and document rules
// stop at the boundary (engine `shadow_tree.rs`: `:host`, `::slotted`).
// What this is not, yet: `::part` is not understood. `slotchange` does not
// fire. Manual slot assignment (`slot.assign()`) assigns nothing.
// `adoptedStyleSheets` is installed by web_cssom.js, which loads next, and
// the engine applies that CSS with this tree's `<style>` elements.
// composedPath() does not hide the inside of a closed shadow root from
// listeners outside it.
//
// The event hooks are installed only once a document has a shadow root
// (attachShadow, or a declarative one when the document is bound).
(function (g) {
    var HOOKS = g.__rkDomHooks;
    var document = g.document;
    var S = g.__rkShadowInternals;
    delete g.__rkShadowInternals;
    if (!S || !HOOKS || !document || !g.Element || !g.ShadowRoot || !g.DocumentFragment) return;
    var ShadowRoot = g.ShadowRoot;

    function domError(message, name) {
        return typeof g.DOMException === 'function' ? new g.DOMException(message, name) : (function () { var e = new Error(message); e.name = name; return e; })();
    }
    function method(obj, name, fn) {
        Object.defineProperty(obj, name, { value: fn, writable: true, configurable: true, enumerable: true });
    }
    function getter(obj, name, fn, set) {
        Object.defineProperty(obj, name, { get: fn, set: set, configurable: true, enumerable: true });
    }

    // A shadow root's "mode delegatesFocus slotAssignment", from the DOM;
    // null for any other node.
    function initOf(n) {
        if (!n || n.nodeType !== 11) return null;
        var init = S.call('init', n);
        if (init === null) return null;
        var p = init.split(' ');
        return { mode: p[0], delegatesFocus: p[1] === '1', slotAssignment: p[2] };
    }
    function hostOf(n) { return n && n.nodeType === 11 ? S.node('host', n) : null; }

    // ---- the tree
    function rootNode(n) { while (n.parentNode) n = n.parentNode; return n; }
    // `a` is a shadow-including inclusive ancestor of `b`.
    function includes(a, b) {
        for (var n = b; n;) {
            if (n === a) return true;
            n = n.parentNode || hostOf(n);
        }
        return false;
    }
    // DOM §2.7 "retarget".
    function retarget(a, b) {
        for (;;) {
            var r = rootNode(a);
            var host = hostOf(r);
            if (!host) return a;
            if (b && includes(r, b)) return a;
            a = host;
        }
    }
    var installed = false;
    function installHooks() {
        if (installed) return;
        installed = true;
        HOOKS.hostOf = hostOf;
        HOOKS.shadowParent = function (n, event) { return event.composed ? hostOf(n) : null; };
        HOOKS.retarget = retarget;
    }
    // A newly bound document may come with declarative shadow roots.
    var reset = g.__rustkit_dom_reset;
    if (typeof reset === 'function') {
        g.__rustkit_dom_reset = function (newGen) {
            var r = reset.apply(this, arguments);
            if (S.call('any', document) === true) installHooks();
            return r;
        };
    }

    // ---- attachShadow, shadowRoot
    var E = g.Element.prototype;
    method(E, 'attachShadow', function attachShadow(init) {
        if (init === null || typeof init !== 'object') {
            throw new TypeError("Failed to execute 'attachShadow' on 'Element': The provided value is not of type 'ShadowRootInit'.");
        }
        var mode = String(init.mode);
        if (mode !== 'open' && mode !== 'closed') {
            throw new TypeError("Failed to execute 'attachShadow' on 'Element': The provided value '" + mode + "' is not a valid enum value of type ShadowRootMode.");
        }
        var flags = (init.delegatesFocus ? 'd' : '') + (init.slotAssignment === 'manual' ? 'm' : '');
        var id = S.call('attach', this, mode, flags);
        if (typeof id !== 'number') {
            var why = S.call('root', this) !== null
                ? 'Shadow root cannot be created on a host which already hosts a shadow tree.'
                : 'This element does not support attachShadow';
            throw domError("Failed to execute 'attachShadow' on 'Element': " + why, 'NotSupportedError');
        }
        installHooks();
        return S.node('root', this);
    });
    getter(E, 'shadowRoot', function () {
        var r = S.node('root', this);
        return r && initOf(r).mode === 'open' ? r : null;
    });

    // ---- ShadowRoot
    var P = ShadowRoot.prototype;
    function rec(self, name) {
        var r = self != null && typeof self === 'object' && self.nodeType === 11 ? initOf(self) : null;
        if (!r) throw new TypeError("Failed to read the '" + name + "' property from 'ShadowRoot': Illegal invocation");
        return r;
    }
    getter(P, 'host', function () { rec(this, 'host'); return hostOf(this); });
    getter(P, 'mode', function () { return rec(this, 'mode').mode; });
    getter(P, 'delegatesFocus', function () { return rec(this, 'delegatesFocus').delegatesFocus; });
    getter(P, 'slotAssignment', function () { return rec(this, 'slotAssignment').slotAssignment; });
    getter(P, 'clonable', function () { rec(this, 'clonable'); return false; });
    getter(P, 'serializable', function () { rec(this, 'serializable'); return false; });
    getter(P, 'activeElement', function () { rec(this, 'activeElement'); return null; });
    var sheets = new WeakMap();
    getter(P, 'adoptedStyleSheets', function () {
        rec(this, 'adoptedStyleSheets');
        var s = sheets.get(this);
        if (!s) { s = []; sheets.set(this, s); }
        return s;
    }, function (v) {
        rec(this, 'adoptedStyleSheets');
        sheets.set(this, Array.prototype.slice.call(v));
    });
    function escapeText(s) { return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;'); }
    function serialize(node) {
        switch (node.nodeType) {
            case 1: return node.outerHTML;
            case 3: return escapeText(node.data);
            case 8: return '<!--' + node.data + '-->';
            default: return '';
        }
    }
    getter(P, 'innerHTML', function () {
        rec(this, 'innerHTML');
        return Array.prototype.map.call(this.childNodes, serialize).join('');
    }, function (html) {
        rec(this, 'innerHTML');
        var tpl = document.createElement('template');
        tpl.innerHTML = String(html);
        this.replaceChildren.apply(this, Array.prototype.slice.call(tpl.content.childNodes));
    });
    method(P, 'getHTML', function getHTML() { return this.innerHTML; });
    method(P, 'setHTMLUnsafe', function setHTMLUnsafe(html) { this.innerHTML = html; });
    method(P, 'getSelection', function getSelection() { return g.getSelection ? g.getSelection() : null; });
    Object.defineProperty(P, Symbol.toStringTag, { value: 'ShadowRoot', configurable: true });
    // The elements under `node`, in tree order (a plain walk, so nothing here
    // depends on the selector matcher).
    function elementsUnder(node, out) {
        var kids = node.children;
        for (var i = 0; i < kids.length; i++) { out.push(kids[i]); elementsUnder(kids[i], out); }
        return out;
    }
    // getElementById on a fragment.
    if (typeof g.DocumentFragment.prototype.getElementById !== 'function') {
        method(g.DocumentFragment.prototype, 'getElementById', function getElementById(id) {
            id = String(id);
            var all = elementsUnder(this, []);
            for (var i = 0; i < all.length; i++) if (all[i].id === id) return all[i];
            return null;
        });
    }

    // ---- slots: assignment is the DOM's (the one the flat tree uses).
    var HTMLSlotElement = g.HTMLSlotElement;
    function slotName(slot) { return slot.getAttribute('name') || ''; }
    function inShadowTree(n) { return initOf(rootNode(n)) !== null; }
    function assigned(slot) { return S.nodes('assigned', slot); }
    function flattened(slot, out) {
        var list = assigned(slot);
        if (!list.length) list = Array.prototype.slice.call(slot.childNodes);   // fallback content
        list.forEach(function (n) {
            if (n.nodeType === 1 && n.localName === 'slot' && inShadowTree(n)) flattened(n, out);
            else out.push(n);
        });
        return out;
    }
    if (HTMLSlotElement) {
        var SP = HTMLSlotElement.prototype;
        getter(SP, 'name', function () { return slotName(this); }, function (v) { this.setAttribute('name', v); });
        method(SP, 'assignedNodes', function assignedNodes(options) {
            return options && options.flatten ? flattened(this, []) : assigned(this);
        });
        method(SP, 'assignedElements', function assignedElements(options) {
            var list = options && options.flatten ? flattened(this, []) : assigned(this);
            return list.filter(function (n) { return n.nodeType === 1; });
        });
        method(SP, 'assign', function assign() {});
    }
    // Element.assignedSlot and Text.assignedSlot: only through an open shadow root.
    function assignedSlot() {
        var slot = S.node('slot', this);
        return slot && initOf(rootNode(slot)).mode === 'open' ? slot : null;
    }
    getter(E, 'assignedSlot', assignedSlot);
    if (g.Text) getter(g.Text.prototype, 'assignedSlot', assignedSlot);
})(typeof globalThis === 'object' ? globalThis : this);
