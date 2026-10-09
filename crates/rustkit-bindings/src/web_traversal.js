// DOM Traversal (DOM §6): NodeFilter, TreeWalker, NodeIterator and
// document.createTreeWalker / createNodeIterator.
//
// Why: polyfills and libraries (YouTube's webcomponents-sd.js, sanitizers,
// text-walkers) read `NodeFilter.SHOW_ELEMENT` at load and walk the tree with
// a TreeWalker; `NodeFilter` was not defined, so the script threw at once.
//
// Limits: a NodeIterator does not adjust when nodes are removed during
// iteration (the "pre-removing steps" are not run), and `detach()` is a no-op,
// as the specification now has it.
(function (g) {
    var document = g.document;
    if (!document || !g.Node || !g.Document) return;

    var SHOW = {
        SHOW_ALL: 0xFFFFFFFF, SHOW_ELEMENT: 0x1, SHOW_ATTRIBUTE: 0x2, SHOW_TEXT: 0x4, SHOW_CDATA_SECTION: 0x8,
        SHOW_ENTITY_REFERENCE: 0x10, SHOW_ENTITY: 0x20, SHOW_PROCESSING_INSTRUCTION: 0x40, SHOW_COMMENT: 0x80,
        SHOW_DOCUMENT: 0x100, SHOW_DOCUMENT_TYPE: 0x200, SHOW_DOCUMENT_FRAGMENT: 0x400, SHOW_NOTATION: 0x800
    };
    var FILTER_ACCEPT = 1, FILTER_REJECT = 2, FILTER_SKIP = 3;

    if (typeof g.NodeFilter !== 'object' && typeof g.NodeFilter !== 'function') {
        var NodeFilter = {};
        Object.keys(SHOW).forEach(function (k) { Object.defineProperty(NodeFilter, k, { value: SHOW[k], enumerable: true }); });
        Object.defineProperty(NodeFilter, 'FILTER_ACCEPT', { value: FILTER_ACCEPT, enumerable: true });
        Object.defineProperty(NodeFilter, 'FILTER_REJECT', { value: FILTER_REJECT, enumerable: true });
        Object.defineProperty(NodeFilter, 'FILTER_SKIP', { value: FILTER_SKIP, enumerable: true });
        Object.defineProperty(NodeFilter, Symbol.toStringTag, { value: 'NodeFilter' });
        Object.defineProperty(g, 'NodeFilter', { value: NodeFilter, writable: true, configurable: true, enumerable: false });
    }

    function illegal() { throw new TypeError('Illegal constructor'); }
    function iface(name, proto) {
        var C = function () { illegal(); };
        Object.defineProperty(C, 'name', { value: name });
        C.prototype = proto;
        Object.defineProperty(proto, 'constructor', { value: C, writable: true, configurable: true });
        Object.defineProperty(proto, Symbol.toStringTag, { value: name, configurable: true });
        if (typeof g[name] !== 'function') Object.defineProperty(g, name, { value: C, writable: true, configurable: true, enumerable: false });
    }

    // Run the filter on `node` (DOM §6.1 "filter"): ACCEPT, REJECT or SKIP.
    function accept(traversal, node) {
        if (traversal._active) throw new g.DOMException("Failed to execute 'acceptNode': the filter is already running.", 'InvalidStateError');
        var bit = 1 << (node.nodeType - 1);
        if (!(traversal.whatToShow & bit)) return FILTER_SKIP;
        var f = traversal.filter;
        if (f === null) return FILTER_ACCEPT;
        traversal._active = true;
        try {
            return (typeof f === 'function' ? f(node) : f.acceptNode(node)) | 0;
        } finally {
            traversal._active = false;
        }
    }
    function make(Ctor, root, whatToShow, filter) {
        if (root === null || typeof root !== 'object' || typeof root.nodeType !== 'number') {
            throw new TypeError("Failed to execute 'create" + (Ctor === 'TreeWalker' ? 'TreeWalker' : 'NodeIterator') + "' on 'Document': parameter 1 is not of type 'Node'.");
        }
        var o = Object.create(Ctor === 'TreeWalker' ? TW : NI);
        Object.defineProperty(o, 'root', { value: root, enumerable: true });
        Object.defineProperty(o, 'whatToShow', { value: whatToShow === undefined ? 0xFFFFFFFF : (whatToShow >>> 0), enumerable: true });
        var fl = filter === undefined ? null : filter;
        if (fl !== null && typeof fl !== 'function' && typeof fl.acceptNode !== 'function') {
            throw new TypeError("Failed to execute 'create" + Ctor + "': The provided callback is not a function.");
        }
        Object.defineProperty(o, 'filter', { value: fl, enumerable: true });
        Object.defineProperty(o, '_active', { value: false, writable: true });
        return o;
    }

    // ---- TreeWalker
    var TW = (typeof g.TreeWalker === 'function' && g.TreeWalker.prototype) || {};
    if (typeof g.TreeWalker !== 'function') iface('TreeWalker', TW);
    function twInit(w) { if (!('currentNode' in w)) w.currentNode = w.root; }
    function twParent(w) {
        var n = w.currentNode;
        while (n !== null && n !== w.root) {
            n = n.parentNode;
            if (n !== null && accept(w, n) === FILTER_ACCEPT) { w.currentNode = n; return n; }
        }
        return null;
    }
    function twChild(w, first) {
        var node = first ? w.currentNode.firstChild : w.currentNode.lastChild;
        while (node !== null) {
            var r = accept(w, node);
            if (r === FILTER_ACCEPT) { w.currentNode = node; return node; }
            if (r === FILTER_SKIP) {
                var child = first ? node.firstChild : node.lastChild;
                if (child !== null) { node = child; continue; }
            }
            while (node !== null) {
                var sibling = first ? node.nextSibling : node.previousSibling;
                if (sibling !== null) { node = sibling; break; }
                var parent = node.parentNode;
                if (parent === null || parent === w.root || parent === w.currentNode) return null;
                node = parent;
            }
        }
        return null;
    }
    function twSibling(w, next) {
        var node = w.currentNode;
        if (node === w.root) return null;
        for (;;) {
            var sibling = next ? node.nextSibling : node.previousSibling;
            while (sibling !== null) {
                node = sibling;
                var r = accept(w, node);
                if (r === FILTER_ACCEPT) { w.currentNode = node; return node; }
                sibling = next ? node.firstChild : node.lastChild;
                if (r === FILTER_REJECT || sibling === null) sibling = next ? node.nextSibling : node.previousSibling;
            }
            node = node.parentNode;
            if (node === null || node === w.root) return null;
            if (accept(w, node) === FILTER_ACCEPT) return null;
        }
    }
    function mt(proto, name, fn) { Object.defineProperty(proto, name, { value: fn, writable: true, configurable: true, enumerable: true }); }
    mt(TW, 'parentNode', function parentNode() { twInit(this); return twParent(this); });
    mt(TW, 'firstChild', function firstChild() { twInit(this); return twChild(this, true); });
    mt(TW, 'lastChild', function lastChild() { twInit(this); return twChild(this, false); });
    mt(TW, 'nextSibling', function nextSibling() { twInit(this); return twSibling(this, true); });
    mt(TW, 'previousSibling', function previousSibling() { twInit(this); return twSibling(this, false); });
    mt(TW, 'nextNode', function nextNode() {
        twInit(this);
        var node = this.currentNode, result = FILTER_ACCEPT;
        for (;;) {
            while (result !== FILTER_REJECT && node.firstChild !== null) {
                node = node.firstChild;
                result = accept(this, node);
                if (result === FILTER_ACCEPT) { this.currentNode = node; return node; }
            }
            var temp = node, sibling = null;
            while (temp !== null) {
                if (temp === this.root) return null;
                sibling = temp.nextSibling;
                if (sibling !== null) { node = sibling; break; }
                temp = temp.parentNode;
            }
            if (temp === null) return null;
            result = accept(this, node);
            if (result === FILTER_ACCEPT) { this.currentNode = node; return node; }
        }
    });
    mt(TW, 'previousNode', function previousNode() {
        twInit(this);
        var node = this.currentNode;
        while (node !== this.root) {
            var sibling = node.previousSibling;
            while (sibling !== null) {
                node = sibling;
                var result = accept(this, node);
                while (result !== FILTER_REJECT && node.lastChild !== null) {
                    node = node.lastChild;
                    result = accept(this, node);
                }
                if (result === FILTER_ACCEPT) { this.currentNode = node; return node; }
                sibling = node.previousSibling;
            }
            if (node === this.root || node.parentNode === null) return null;
            node = node.parentNode;
            if (accept(this, node) === FILTER_ACCEPT) { this.currentNode = node; return node; }
        }
        return null;
    });

    // ---- NodeIterator
    var NI = (typeof g.NodeIterator === 'function' && g.NodeIterator.prototype) || {};
    if (typeof g.NodeIterator !== 'function') iface('NodeIterator', NI);
    function niInit(it) {
        if (!('referenceNode' in it)) { it.referenceNode = it.root; it.pointerBeforeReferenceNode = true; }
    }
    function niTraverse(it, next) {
        niInit(it);
        var node = it.referenceNode, before = it.pointerBeforeReferenceNode;
        for (;;) {
            if (next) {
                if (!before) {
                    // The next node in tree order after `node`, within root.
                    var n = node.firstChild;
                    if (n === null) {
                        var t = node;
                        while (t !== null && t !== it.root && t.nextSibling === null) t = t.parentNode;
                        n = (t === null || t === it.root) ? null : t.nextSibling;
                    }
                    if (n === null) return null;
                    node = n;
                } else before = false;
            } else {
                if (before) {
                    if (node === it.root) return null;
                    var p = node.previousSibling;
                    if (p === null) node = node.parentNode;
                    else { node = p; while (node.lastChild !== null) node = node.lastChild; }
                    if (node === null) return null;
                } else before = true;
            }
            if (accept(it, node) === FILTER_ACCEPT) {
                it.referenceNode = node;
                it.pointerBeforeReferenceNode = before;
                return node;
            }
        }
    }
    mt(NI, 'nextNode', function nextNode() { return niTraverse(this, true); });
    mt(NI, 'previousNode', function previousNode() { return niTraverse(this, false); });
    mt(NI, 'detach', function detach() {});

    function def(proto, name, fn) {
        if (typeof proto[name] === 'function') return;
        Object.defineProperty(proto, name, { value: fn, writable: true, configurable: true, enumerable: true });
    }
    def(g.Document.prototype, 'createTreeWalker', function createTreeWalker(root, whatToShow, filter) {
        var w = make('TreeWalker', root, whatToShow, filter);
        w.currentNode = root;
        return w;
    });
    def(g.Document.prototype, 'createNodeIterator', function createNodeIterator(root, whatToShow, filter) {
        var it = make('NodeIterator', root, whatToShow, filter);
        it.referenceNode = root;
        it.pointerBeforeReferenceNode = true;
        return it;
    });
})(typeof globalThis === 'object' ? globalThis : this);
