// MutationObserver that delivers (DOM §4.3). It replaces the inert stub in
// web_observers.js, whose callback never ran and whose takeRecords() was
// always empty, so frameworks, consent scripts and widgets waiting on a
// mutation waited forever.
//
// Interception: every DOM write the bindings expose ends in one of the two
// host calls in dom.rs, `write` (insert, remove) and `setData` (attributes,
// character data, textContent/innerHTML/innerText). Each asks the
// `__rkDomHooks.mutation` hook, which this file installs at the first
// observe(): until then a write pays that one check. The custom-element
// wrappers (web_components.js) sit above the same calls, on the public
// methods, and are unchanged; their reactions run as before.
//
// Delivery: records queue per observer, and one microtask after the script
// that wrote calls each observer that has records, in creation order, with
// all of them.
//
// Stated limits: insertAdjacentHTML queues one record per inserted node
// (the browser queues one); a template's innerHTML is recorded on the
// template rather than on its content; observers are held strongly while
// they observe something.
(function (g) {
    var HOOKS = g.__rkDomHooks, Node = g.Node;
    if (!HOOKS || !Node) return;
    var HTML_NS = 'http://www.w3.org/1999/xhtml';

    function report(e) {
        try { if (g.__rustkit_errors) g.__rustkit_errors.push(String(e)); } catch (_) {}
    }
    function slice(list) { return Array.prototype.slice.call(list); }

    var observers = [];         // the observers with registrations, by creation
    var regs = new WeakMap();   // node -> [{ observer, options, source }]
    var STATE = typeof Symbol === 'function' ? Symbol('rustkit.mo') : '__rkMO';
    var nextId = 0, queued = false;
    var depth = 0, merging = null;   // batch(): target -> the records made for it

    function MutationRecord() { throw new TypeError('Illegal constructor'); }
    Object.defineProperty(MutationRecord.prototype, Symbol.toStringTag, { value: 'MutationRecord', configurable: true });
    function nodeList(nodes) {
        var l = Object.create(g.NodeList.prototype);
        nodes.forEach(function (n, i) { l[i] = n; });
        Object.defineProperty(l, 'length', { value: nodes.length });
        return l;
    }
    function record(type, target, f, oldValue) {
        var r = Object.create(MutationRecord.prototype);
        r.type = type;
        r.target = target;
        r.addedNodes = nodeList(f.added || []);
        r.removedNodes = nodeList(f.removed || []);
        r.previousSibling = f.prev || null;
        r.nextSibling = f.next || null;
        r.attributeName = type === 'attributes' ? f.name : null;
        r.attributeNamespace = null;
        r.oldValue = oldValue;
        return r;
    }

    // DOM "queue a mutation record": every observer registered on target or
    // (with subtree) an ancestor that wants this type gets its own record.
    function queue(type, target, f) {
        if (type === 'childList' && merging) {
            var prior = merging.get(target);
            if (prior) {
                prior.added = prior.added.concat(f.added || []);
                prior.removed = prior.removed.concat(f.removed || []);
                prior.records.forEach(function (r) {
                    r.addedNodes = nodeList(prior.added);
                    r.removedNodes = nodeList(prior.removed);
                    r.nextSibling = f.next || null;
                });
                return;
            }
        }
        var interested = [], olds = [];
        for (var n = target; n; n = n.parentNode) {
            (regs.get(n) || []).forEach(function (reg) {
                var o = reg.options;
                if (n !== target && !o.subtree) return;
                if (type === 'attributes' && (!o.attributes || (o.attributeFilter && o.attributeFilter.indexOf(f.name) < 0))) return;
                if (type === 'characterData' && !o.characterData) return;
                if (type === 'childList' && !o.childList) return;
                var i = interested.indexOf(reg.observer);
                if (i < 0) { i = interested.push(reg.observer) - 1; olds[i] = null; }
                if ((type === 'attributes' && o.attributeOldValue) || (type === 'characterData' && o.characterDataOldValue)) {
                    olds[i] = f.oldValue;
                }
            });
        }
        var made = interested.map(function (mo, i) {
            var r = record(type, target, f, olds[i]);
            mo[STATE].records.push(r);
            return r;
        });
        if (type === 'childList' && merging) {
            merging.set(target, { records: made, added: f.added || [], removed: f.removed || [] });
        }
        if (made.length && !queued) {
            queued = true;
            Promise.resolve().then(notify);
        }
    }

    // DOM "notify mutation observers".
    function notify() {
        queued = false;
        observers.slice().forEach(function (mo) {
            var st = mo[STATE], records = st.records.splice(0);
            dropTransients(st, null);
            if (records.length) {
                try { st.callback.call(mo, records, mo); } catch (e) { report(e); }
            }
        });
    }

    // A node leaving `parent` stays observed by the subtree observers above
    // it (transient registrations) until their next delivery.
    function removed(parent, node, prev, next) {
        stayObserved(parent, node);
        queue('childList', parent, { removed: [node], prev: prev, next: next });
    }
    function stayObserved(parent, node) {
        for (var n = parent; n; n = n.parentNode) {
            (regs.get(n) || []).forEach(function (reg) {
                if (!reg.options.subtree) return;
                var list = regs.get(node) || [];
                regs.set(node, list);
                list.push({ observer: reg.observer, options: reg.options, source: reg.source || reg });
                reg.observer[STATE].transients.push(node);
            });
        }
    }
    function dropTransients(st, source) {
        st.transients.forEach(function (node) {
            var list = regs.get(node);
            if (list) regs.set(node, list.filter(function (r) { return !r.source || r.observer[STATE] !== st || (source && r.source !== source); }));
        });
        if (!source) st.transients = [];
    }

    var hook = {
        // Before a tree write; answers what to record once it is done.
        tree: function (op, parent, node, child) {
            if (!observers.length) return null;
            if (op === 'remove') {
                var prev = node.previousSibling, next = node.nextSibling;
                return function () { removed(parent, node, prev, next); };
            }
            var fragment = node.nodeType === 11;
            var moved = fragment ? slice(node.childNodes) : [node];
            var old = fragment ? null : node.parentNode;
            var oldPrev = old && node.previousSibling, oldNext = old && node.nextSibling;
            var ref = child === node ? node.nextSibling : child || null;
            var before = ref ? ref.previousSibling : parent.lastChild;
            if (before === node) before = node.previousSibling;
            return function () {
                if (old) removed(old, node, oldPrev, oldNext);
                if (!moved.length) return;
                if (fragment) queue('childList', node, { removed: moved });
                queue('childList', parent, { added: moved, prev: before, next: ref });
            };
        },
        // Before a data write (dom.rs setData).
        data: function (o, op, name) {
            if (!observers.length) return null;
            var t = o.nodeType;
            if (op === 'setAttr' || op === 'removeAttr') {
                if (t !== 1) return null;
                var key = o.namespaceURI === HTML_NS ? String(name).toLowerCase() : String(name);
                var was = o.getAttribute(key);
                if (op === 'removeAttr' && was === null) return null;
                return function () { queue('attributes', o, { name: key, oldValue: was }); };
            }
            if (op === 'setText' && (t === 3 || t === 7 || t === 8)) {
                var data = o.data;
                return function () { queue('characterData', o, { oldValue: data }); };
            }
            if ((op === 'setText' || op === 'setHTML' || op === 'setInnerText') && (t === 1 || t === 11)) {
                var gone = slice(o.childNodes);
                return function () {
                    var added = slice(o.childNodes);
                    if (!added.length && !gone.length) return;
                    gone.forEach(function (n) { stayObserved(o, n); });
                    queue('childList', o, { added: added, removed: gone });
                };
            }
            return null;
        },
        batch: function (delta) {
            depth += delta;
            merging = depth > 0 ? merging || new Map() : null;
        }
    };

    function stateOf(mo, method) {
        var st = mo != null ? mo[STATE] : undefined;
        if (!st) throw new TypeError("Failed to execute '" + method + "' on 'MutationObserver': Illegal invocation");
        return st;
    }
    function track(mo) {
        if (observers.indexOf(mo) >= 0) return;
        var id = mo[STATE].id, i = 0;
        while (i < observers.length && observers[i][STATE].id < id) i++;
        observers.splice(i, 0, mo);
    }

    function MutationObserver(callback) {
        if (!(this instanceof MutationObserver)) {
            throw new TypeError("Failed to construct 'MutationObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        }
        if (typeof callback !== 'function') {
            throw new TypeError("Failed to construct 'MutationObserver': The callback provided as parameter 1 is not a function.");
        }
        Object.defineProperty(this, STATE, { value: { id: nextId++, callback: callback, records: [], nodes: [], transients: [] } });
    }
    MutationObserver.prototype.observe = function (target, options) {
        var st = stateOf(this, 'observe');
        function fail(why) { throw new TypeError("Failed to execute 'observe' on 'MutationObserver': " + why); }
        if (!(target instanceof Node)) fail("parameter 1 is not of type 'Node'.");
        var o = options == null ? {} : options;
        var attributes = o.attributes, characterData = o.characterData;
        var filter = o.attributeFilter === undefined ? undefined : Array.from(o.attributeFilter, String);
        if (attributes === undefined && (o.attributeOldValue !== undefined || filter !== undefined)) attributes = true;
        if (characterData === undefined && o.characterDataOldValue !== undefined) characterData = true;
        var opts = { childList: !!o.childList, attributes: !!attributes, characterData: !!characterData,
                     subtree: !!o.subtree, attributeOldValue: !!o.attributeOldValue,
                     characterDataOldValue: !!o.characterDataOldValue, attributeFilter: filter };
        if (!opts.childList && !opts.attributes && !opts.characterData) {
            fail("The options object must set at least one of 'attributes', 'characterData', or 'childList' to true.");
        }
        if (opts.attributeOldValue && !opts.attributes) fail("The options object may only set 'attributeOldValue' to true when 'attributes' is true or not present.");
        if (filter && !opts.attributes) fail("The options object may only set 'attributeFilter' when 'attributes' is true or not present.");
        if (opts.characterDataOldValue && !opts.characterData) fail("The options object may only set 'characterDataOldValue' to true when 'characterData' is true or not present.");
        var list = regs.get(target) || [], self = this;
        regs.set(target, list);
        var existing = list.filter(function (r) { return r.observer === self && !r.source; })[0];
        if (existing) {
            dropTransients(st, existing);
            existing.options = opts;
        } else {
            list.push({ observer: this, options: opts, source: null });
            st.nodes.push(target);
        }
        track(this);
        HOOKS.mutation = hook;
    };
    MutationObserver.prototype.disconnect = function () {
        var st = stateOf(this, 'disconnect'), self = this;
        st.nodes.concat(st.transients).forEach(function (n) {
            var list = regs.get(n);
            if (list) regs.set(n, list.filter(function (r) { return r.observer !== self; }));
        });
        st.nodes = [];
        st.transients = [];
        st.records = [];
        var i = observers.indexOf(this);
        if (i >= 0) observers.splice(i, 1);
    };
    MutationObserver.prototype.takeRecords = function () {
        return stateOf(this, 'takeRecords').records.splice(0);
    };
    Object.defineProperty(MutationObserver.prototype, Symbol.toStringTag, { value: 'MutationObserver', configurable: true });

    [['MutationObserver', MutationObserver], ['WebKitMutationObserver', MutationObserver],
     ['MutationRecord', MutationRecord]].forEach(function (p) {
        Object.defineProperty(g, p[0], { value: p[1], writable: true, configurable: true, enumerable: false });
    });
})(globalThis);
