// Shadow DOM, the script-visible half (DOM §4.8, HTML §4.2.2): attachShadow,
// Element.shadowRoot, ShadowRoot (a DocumentFragment with host/mode/
// innerHTML), <slot> assignment (assignedNodes/assignedElements,
// assignedSlot), and the event path through a shadow boundary (composed
// events go on to the host; a listener outside sees the host as the target;
// composedPath()).
//
// What this is not, yet: the shadow tree is held beside the document, not in
// the rendered tree. Nothing in a shadow root is laid out or painted, its
// styles apply to nothing, and the `:host`/`::slotted`/`::part` selectors are
// not understood (the flat tree and style scoping are the next two slices).
// `slotchange` does not fire. composedPath() does not hide the inside of a
// closed shadow root from listeners outside it.
//
// Pure JS over the DOM bindings; it does nothing (no hook is installed) until
// a page calls attachShadow.
(function (g) {
    var HOOKS = g.__rkDomHooks;
    var document = g.document;
    if (!HOOKS || !document || !g.Element || !g.ShadowRoot || !g.DocumentFragment) return;
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

    var recordOf = new WeakMap();   // shadow root -> { host, mode, delegatesFocus, slotAssignment }
    var rootOf = new WeakMap();     // host -> shadow root
    var installed = false;

    // Elements that can host a shadow root: autonomous custom elements and
    // these (DOM §4.2.14 "valid shadow host name").
    var HOSTS = { article: 1, aside: 1, blockquote: 1, body: 1, div: 1, footer: 1, h1: 1, h2: 1, h3: 1, h4: 1, h5: 1, h6: 1,
                  header: 1, main: 1, nav: 1, p: 1, section: 1, span: 1 };
    var RESERVED = { 'annotation-xml': 1, 'color-profile': 1, 'font-face': 1, 'font-face-src': 1, 'font-face-uri': 1,
                     'font-face-format': 1, 'font-face-name': 1, 'missing-glyph': 1 };
    function canHost(el) {
        var name = String(el.localName).toLowerCase();
        if (HOSTS[name] === 1) return true;
        return /^[a-z][^A-Z]*-/.test(name) && !RESERVED[name] && name.indexOf(' ') < 0;
    }

    // ---- the tree
    function rootNode(n) { while (n.parentNode) n = n.parentNode; return n; }
    // `a` is a shadow-including inclusive ancestor of `b`.
    function includes(a, b) {
        for (var n = b; n;) {
            if (n === a) return true;
            var r = recordOf.get(n);
            n = n.parentNode || (r ? r.host : null);
        }
        return false;
    }
    // DOM §2.7 "retarget".
    function retarget(a, b) {
        for (;;) {
            var r = rootNode(a);
            var rec = recordOf.get(r);
            if (!rec) return a;
            if (b && includes(r, b)) return a;
            a = rec.host;
        }
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
        if (!canHost(this)) {
            throw domError("Failed to execute 'attachShadow' on 'Element': This element does not support attachShadow", 'NotSupportedError');
        }
        if (rootOf.has(this)) {
            throw domError("Failed to execute 'attachShadow' on 'Element': Shadow root cannot be created on a host which already hosts a shadow tree.", 'NotSupportedError');
        }
        var root = document.createDocumentFragment();
        Object.setPrototypeOf(root, ShadowRoot.prototype);
        recordOf.set(root, {
            host: this, mode: mode, delegatesFocus: !!init.delegatesFocus,
            slotAssignment: init.slotAssignment === 'manual' ? 'manual' : 'named'
        });
        rootOf.set(this, root);
        if (!installed) {
            installed = true;
            HOOKS.hostOf = function (n) { var r = recordOf.get(n); return r ? r.host : null; };
            HOOKS.shadowParent = function (n, event) { var r = recordOf.get(n); return r && event.composed ? r.host : null; };
            HOOKS.retarget = retarget;
        }
        return root;
    });
    getter(E, 'shadowRoot', function () {
        var r = rootOf.get(this);
        return r && recordOf.get(r).mode === 'open' ? r : null;
    });

    // ---- ShadowRoot
    var P = ShadowRoot.prototype;
    function rec(self, name) {
        var r = recordOf.get(self);
        if (!r) throw new TypeError("Failed to read the '" + name + "' property from 'ShadowRoot': Illegal invocation");
        return r;
    }
    getter(P, 'host', function () { return rec(this, 'host').host; });
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

    // ---- slots
    var HTMLSlotElement = g.HTMLSlotElement;
    function slotName(slot) { return slot.getAttribute('name') || ''; }
    function nameOf(slottable) { return slottable.nodeType === 1 ? (slottable.getAttribute('slot') || '') : ''; }
    // The first slot in `root`, in tree order, called `name`.
    function findSlot(root, name) {
        var all = elementsUnder(root, []);
        for (var i = 0; i < all.length; i++) {
            if (all[i].localName === 'slot' && slotName(all[i]) === name) return all[i];
        }
        return null;
    }
    function hostRecord(slot) {
        var r = rootNode(slot);
        return recordOf.get(r) ? { root: r, host: recordOf.get(r).host } : null;
    }
    // What the host's light children give this slot, in tree order.
    function assigned(slot) {
        var hr = hostRecord(slot);
        if (!hr) return [];
        var out = [], kids = hr.host.childNodes;
        for (var i = 0; i < kids.length; i++) {
            var k = kids[i];
            if (k.nodeType !== 1 && k.nodeType !== 3) continue;
            if (findSlot(hr.root, nameOf(k)) === slot) out.push(k);
        }
        return out;
    }
    function flattened(slot, out) {
        var list = assigned(slot);
        if (!list.length) list = Array.prototype.slice.call(slot.childNodes);   // fallback content
        list.forEach(function (n) {
            if (n.nodeType === 1 && n.localName === 'slot' && hostRecord(n)) flattened(n, out);
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
        var p = this.parentNode;
        var root = p && rootOf.get(p);
        if (!root || recordOf.get(root).mode !== 'open') return null;
        return findSlot(root, nameOf(this));
    }
    getter(E, 'assignedSlot', assignedSlot);
    if (g.Text) getter(g.Text.prototype, 'assignedSlot', assignedSlot);
})(typeof globalThis === 'object' ? globalThis : this);
