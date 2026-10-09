// Observer interfaces and requestIdleCallback.
//
// These exist so pages that construct them (almost every modern page does,
// during start-up) carry on instead of dying on a ReferenceError. They are
// honest about what they do: observers accept observe/unobserve/disconnect
// and report no records, and never call their callback. A RustKit view has
// no scrolling and no user resizing, and does not yet feed DOM mutation or
// layout geometry into script; wiring real records belongs with those
// features. Each name is only defined when nothing has defined it.
// With a document, MutationObserver is replaced by the one that records
// and delivers (web_mutation_observer.js), as the IntersectionObserver and
// ResizeObserver are (web_observers_live.js).
(function (g) {
    function def(name, value) {
        if (g[name] === undefined) {
            Object.defineProperty(g, name, { value: value, writable: true, configurable: true, enumerable: false });
        }
    }
    function requireCallback(ctor, cb) {
        if (typeof cb !== 'function') {
            throw new TypeError("Failed to construct '" + ctor + "': The callback provided as parameter 1 is not a function.");
        }
    }
    function requireNew(self, ctor) {
        return self instanceof ctor;
    }

    // ---- MutationObserver (DOM §4.3)
    function MutationObserver(callback) {
        if (!requireNew(this, MutationObserver)) throw new TypeError("Failed to construct 'MutationObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        requireCallback('MutationObserver', callback);
        Object.defineProperty(this, '_targets', { value: [] });
    }
    MutationObserver.prototype.observe = function (target, options) {
        if (target === null || typeof target !== 'object') {
            throw new TypeError("Failed to execute 'observe' on 'MutationObserver': parameter 1 is not of type 'Node'.");
        }
        var o = options || {};
        if (!o.childList && !o.attributes && !o.characterData && !o.attributeOldValue && !o.attributeFilter && !o.characterDataOldValue) {
            throw new TypeError("Failed to execute 'observe' on 'MutationObserver': The options object must set at least one of 'attributes', 'characterData', or 'childList' to true.");
        }
        if (this._targets.indexOf(target) < 0) this._targets.push(target);
    };
    MutationObserver.prototype.disconnect = function () { this._targets.length = 0; };
    MutationObserver.prototype.takeRecords = function () { return []; };
    def('MutationObserver', MutationObserver);
    def('WebKitMutationObserver', MutationObserver);

    // ---- IntersectionObserver (Intersection Observer §3)
    function IntersectionObserver(callback, options) {
        if (!requireNew(this, IntersectionObserver)) throw new TypeError("Failed to construct 'IntersectionObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        requireCallback('IntersectionObserver', callback);
        var o = options || {};
        var t = o.threshold === undefined ? [0] : (Array.isArray(o.threshold) ? o.threshold : [o.threshold]);
        Object.defineProperty(this, 'root', { value: o.root === undefined ? null : o.root, enumerable: true });
        Object.defineProperty(this, 'rootMargin', { value: o.rootMargin === undefined ? '0px 0px 0px 0px' : String(o.rootMargin), enumerable: true });
        Object.defineProperty(this, 'thresholds', { value: Object.freeze(t.map(Number).sort(function (a, b) { return a - b; })), enumerable: true });
        Object.defineProperty(this, '_targets', { value: [] });
    }
    IntersectionObserver.prototype.observe = function (target) {
        if (target === null || typeof target !== 'object') {
            throw new TypeError("Failed to execute 'observe' on 'IntersectionObserver': parameter 1 is not of type 'Element'.");
        }
        if (this._targets.indexOf(target) < 0) this._targets.push(target);
    };
    IntersectionObserver.prototype.unobserve = function (target) {
        var i = this._targets.indexOf(target);
        if (i >= 0) this._targets.splice(i, 1);
    };
    IntersectionObserver.prototype.disconnect = function () { this._targets.length = 0; };
    IntersectionObserver.prototype.takeRecords = function () { return []; };
    def('IntersectionObserver', IntersectionObserver);

    // ---- ResizeObserver (Resize Observer §2)
    function ResizeObserver(callback) {
        if (!requireNew(this, ResizeObserver)) throw new TypeError("Failed to construct 'ResizeObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        requireCallback('ResizeObserver', callback);
        Object.defineProperty(this, '_targets', { value: [] });
    }
    ResizeObserver.prototype.observe = function (target) {
        if (target === null || typeof target !== 'object') {
            throw new TypeError("Failed to execute 'observe' on 'ResizeObserver': parameter 1 is not of type 'Element'.");
        }
        if (this._targets.indexOf(target) < 0) this._targets.push(target);
    };
    ResizeObserver.prototype.unobserve = function (target) {
        var i = this._targets.indexOf(target);
        if (i >= 0) this._targets.splice(i, 1);
    };
    ResizeObserver.prototype.disconnect = function () { this._targets.length = 0; };
    def('ResizeObserver', ResizeObserver);

    // ---- PerformanceObserver (Performance Timeline §6)
    function PerformanceObserver(callback) {
        if (!requireNew(this, PerformanceObserver)) throw new TypeError("Failed to construct 'PerformanceObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        requireCallback('PerformanceObserver', callback);
    }
    PerformanceObserver.prototype.observe = function () {};
    PerformanceObserver.prototype.disconnect = function () {};
    PerformanceObserver.prototype.takeRecords = function () { return []; };
    PerformanceObserver.supportedEntryTypes = Object.freeze([]);
    def('PerformanceObserver', PerformanceObserver);

    // ---- requestIdleCallback (Cooperative Scheduling §4). A view with no
    // input is always idle, so the callback runs on the next timer turn with
    // a deadline that never reports a shortage.
    var idleId = 0, idleTimers = {};
    def('requestIdleCallback', function requestIdleCallback(callback, options) {
        if (typeof callback !== 'function') {
            throw new TypeError("Failed to execute 'requestIdleCallback' on 'Window': The callback provided as parameter 1 is not a function.");
        }
        var id = ++idleId, start = Date.now();
        idleTimers[id] = g.setTimeout(function () {
            delete idleTimers[id];
            callback({
                didTimeout: false,
                timeRemaining: function () { return Math.max(0, 50 - (Date.now() - start)); }
            });
        }, 1);
        return id;
    });
    def('cancelIdleCallback', function cancelIdleCallback(id) {
        if (idleTimers[id] !== undefined) {
            g.clearTimeout(idleTimers[id]);
            delete idleTimers[id];
        }
    });
})(globalThis);
