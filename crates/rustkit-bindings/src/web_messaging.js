// Task-queue messaging (HTML §9.3-§9.4) and cooperative scheduling: the
// MessageEvent constructor, window.postMessage to the same window,
// MessageChannel/MessagePort, and requestIdleCallback. Evaluated after the
// Rust-backed DOM wrappers, the interface objects and the timer queue exist.
//
// A message is a task, never a synchronous call: each delivery is queued
// through the setTimeout(…, 0) captured here (the page lifecycle's virtual
// clock queue), so it runs after the posting script and its microtasks, in
// post order, and interleaves with setTimeout(0) by when each was queued.
// React's scheduler drives its work loop through MessageChannel when it
// exists, so order matters more than breadth here.
//
// Stated limits: same-window only. There are no other windows, iframes or
// workers, so cross-window and cross-frame messaging, and BroadcastChannel,
// are not here. A transferred port stays usable on the sending side (there
// is no other realm to move it to) and a transferred ArrayBuffer is not
// detached. A MessagePort nested inside the message (rather than the
// message itself) is not detected as uncloneable. As with timers, promise
// reactions queued by a delivered message run when the engine's timer turn
// returns, not between two tasks of the same turn.
(function (g) {
    var Event = g.Event, EventTarget = g.EventTarget;
    if (typeof Event !== 'function' || typeof EventTarget !== 'function') return;
    var post = g.setTimeout, cancel = g.clearTimeout;

    function hide(obj, name, value) {
        Object.defineProperty(obj, name, { value: value, writable: true, configurable: true, enumerable: false });
    }
    function tag(C, name) {
        Object.defineProperty(C, 'name', { value: name });
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
    }
    function needNew(self, C, name) {
        if (!(self instanceof C)) {
            throw new TypeError("Failed to construct '" + name + "': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        }
    }
    function cloneError(message) { return new DOMException(message, 'DataCloneError'); }

    // ---- MessagePort (HTML §9.4.4). State lives in a side table so pages
    // see only the standard members.
    var PORTS = new WeakMap(), CONSTRUCTING = false;
    function MessagePort() {
        if (!CONSTRUCTING) throw new TypeError('Illegal constructor');
        PORTS.set(this, { other: null, started: false, closed: false, queue: [], scheduled: 0, onmessage: null, onmessageerror: null });
    }
    Object.setPrototypeOf(MessagePort.prototype, EventTarget.prototype);
    tag(MessagePort, 'MessagePort');
    function state(port, method) {
        var s = PORTS.get(port);
        if (!s) throw new TypeError("Failed to execute '" + method + "' on 'MessagePort': Illegal invocation");
        return s;
    }
    function newPort() {
        CONSTRUCTING = true;
        try { return new MessagePort(); } finally { CONSTRUCTING = false; }
    }

    // A transfer list (a sequence, or `{ transfer }`): the ports it moves.
    // A port listed twice, or the sending port itself, cannot be moved.
    function transferPorts(list, self) {
        var ports = [];
        if (list === undefined || list === null) return ports;
        if (typeof list !== 'object' || typeof list[Symbol.iterator] !== 'function') {
            throw new TypeError("Failed to execute 'postMessage': The transfer list is not a sequence.");
        }
        Array.from(list).forEach(function (item) {
            if (PORTS.has(item)) {
                if (item === self) throw cloneError('Port at index ' + ports.length + ' contains the source port.');
                if (ports.indexOf(item) >= 0) {
                    throw cloneError('Message port at index ' + ports.length + ' is a duplicate of an earlier port.');
                }
                ports.push(item);
            } else if (!(item instanceof ArrayBuffer)) {
                throw cloneError('Value at index ' + ports.length + ' does not have a transferable type.');
            }
        });
        return ports;
    }
    function serialize(message, ports) {
        if (PORTS.has(message) && ports.indexOf(message) < 0) {
            throw cloneError('A MessagePort could not be cloned because it was not transferred.');
        }
        return PORTS.has(message) ? message : g.structuredClone(message);
    }

    // Each message waiting on a started port has one delivery task queued.
    function schedule(port) {
        var s = PORTS.get(port);
        while (s.started && !s.closed && s.scheduled < s.queue.length) {
            s.scheduled++;
            post(function () { deliver(port); }, 0);
        }
    }
    function deliver(port) {
        var s = PORTS.get(port);
        s.scheduled--;
        if (s.closed || !s.queue.length) return;
        var m = s.queue.shift();
        var event = new g.MessageEvent('message', { data: m.data, ports: m.ports });
        event.isTrusted = true;
        port.dispatchEvent(event);
    }

    MessagePort.prototype.postMessage = function postMessage(message, options) {
        var s = state(this, 'postMessage');
        if (arguments.length < 1) {
            throw new TypeError("Failed to execute 'postMessage' on 'MessagePort': 1 argument required, but only 0 present.");
        }
        var list = options;
        if (options && typeof options === 'object' && typeof options[Symbol.iterator] !== 'function') list = options.transfer;
        var ports = transferPorts(list, this);
        var data = serialize(message, ports);
        var target = s.closed ? null : s.other;
        if (!target || PORTS.get(target).closed) return;
        PORTS.get(target).queue.push({ data: data, ports: ports });
        schedule(target);
    };
    MessagePort.prototype.start = function start() {
        var s = state(this, 'start');
        if (s.started || s.closed) return;
        s.started = true;
        schedule(this);
    };
    MessagePort.prototype.close = function close() {
        var s = state(this, 'close');
        if (s.closed) return;
        s.closed = true;
        s.queue.length = 0;
        if (s.other) PORTS.get(s.other).other = null;
        s.other = null;
    };
    ['message', 'messageerror'].forEach(function (type) {
        Object.defineProperty(MessagePort.prototype, 'on' + type, {
            get: function () { return state(this, 'on' + type)['on' + type]; },
            set: function (h) {
                var s = state(this, 'on' + type);
                s['on' + type] = typeof h === 'function' ? h : null;
                // Setting onmessage starts the port (HTML §9.4.4).
                if (type === 'message' && s.onmessage) this.start();
            },
            enumerable: true, configurable: true
        });
    });
    hide(g, 'MessagePort', MessagePort);

    // ---- MessageChannel (HTML §9.4.3): two entangled ports.
    function MessageChannel() {
        needNew(this, MessageChannel, 'MessageChannel');
        var p1 = newPort(), p2 = newPort();
        PORTS.get(p1).other = p2;
        PORTS.get(p2).other = p1;
        hide(this, '__p1', p1);
        hide(this, '__p2', p2);
    }
    tag(MessageChannel, 'MessageChannel');
    Object.defineProperty(MessageChannel.prototype, 'port1', { get: function () { return this.__p1; }, enumerable: true, configurable: true });
    Object.defineProperty(MessageChannel.prototype, 'port2', { get: function () { return this.__p2; }, enumerable: true, configurable: true });
    hide(g, 'MessageChannel', MessageChannel);

    // ---- MessageEvent (HTML §9.2). Replaces the generic one from
    // web_interfaces.js: `ports` holds only MessagePorts, and the legacy
    // initMessageEvent exists. `ports` is a fresh array per event, not the
    // spec's frozen one.
    function portList(ports, where) {
        if (ports === undefined || ports === null) return [];
        if (typeof ports !== 'object' || typeof ports[Symbol.iterator] !== 'function') {
            throw new TypeError(where + "The provided value cannot be converted to a sequence.");
        }
        var list = Array.from(ports);
        list.forEach(function (p, i) {
            if (!PORTS.has(p)) throw new TypeError(where + "Failed to convert value at index " + i + " to 'MessagePort'.");
        });
        return list;
    }
    function MessageEvent(type, init) {
        needNew(this, MessageEvent, 'MessageEvent');
        if (arguments.length < 1) {
            throw new TypeError("Failed to construct 'MessageEvent': 1 argument required, but only 0 present.");
        }
        Event.call(this, type, init);
        var i = init || {};
        this.data = i.data === undefined ? null : i.data;
        this.origin = i.origin === undefined ? '' : String(i.origin);
        this.lastEventId = i.lastEventId === undefined ? '' : String(i.lastEventId);
        this.source = i.source === undefined ? null : i.source;
        this.ports = portList(i.ports, "Failed to construct 'MessageEvent': ");
    }
    MessageEvent.prototype = Object.create(Event.prototype, { constructor: { value: MessageEvent, writable: true, configurable: true } });
    tag(MessageEvent, 'MessageEvent');
    MessageEvent.prototype.initMessageEvent = function (type, bubbles, cancelable, data, origin, lastEventId, source, ports) {
        if (this.eventPhase !== 0) return;
        this.initEvent(type, bubbles, cancelable);
        this.data = data === undefined ? null : data;
        this.origin = origin === undefined ? '' : String(origin);
        this.lastEventId = lastEventId === undefined ? '' : String(lastEventId);
        this.source = source === undefined ? null : source;
        this.ports = portList(ports, "Failed to execute 'initMessageEvent' on 'MessageEvent': ");
    };
    hide(g, 'MessageEvent', MessageEvent);

    // ---- window.postMessage (HTML §9.3.3), same window only.
    // '*' matches any origin; '/' the document's own; anything else must
    // parse as a URL and its origin is compared at delivery. A document with
    // an opaque origin ('null') matches only '*' and '/'.
    hide(g, 'postMessage', function postMessage(message, targetOrigin, transfer) {
        if (arguments.length < 1) {
            throw new TypeError("Failed to execute 'postMessage' on 'Window': 1 argument required, but only 0 present.");
        }
        var target = '/';
        if (targetOrigin !== null && typeof targetOrigin === 'object') {
            if (targetOrigin.targetOrigin !== undefined) target = String(targetOrigin.targetOrigin);
            transfer = targetOrigin.transfer;
        } else if (targetOrigin !== undefined) {
            target = String(targetOrigin);
        }
        var origin = null;
        if (target !== '*' && target !== '/') {
            var u;
            try { u = new URL(target); } catch (e) { u = null; }
            if (!u) throw new DOMException("Failed to execute 'postMessage' on 'Window': Invalid target origin '" + target + "' in a call to 'postMessage'.", 'SyntaxError');
            origin = u.origin;
        }
        var ports = transferPorts(transfer, null);
        var data = serialize(message, ports);
        post(function () {
            var own = g.location ? String(g.location.origin) : 'null';
            if (origin !== null && (own === 'null' || origin !== own)) return;
            var event = new MessageEvent('message', { data: data, origin: own, lastEventId: '', source: g, ports: ports });
            event.isTrusted = true;
            g.dispatchEvent(event);
        }, 0);
    });

    // ---- requestIdleCallback (Cooperative Scheduling §4). Replaces the
    // 1 ms timer in web_observers.js. An idle period starts when a task runs
    // with no other task due now; it runs the callbacks queued before it
    // started, each with a 50 ms deadline from the period's start. A busy
    // queue defers it again behind the tasks now due. With `timeout`, a
    // callback still waiting when that much time has passed (on the
    // virtual clock, or in real time while the queue stays busy) runs with
    // didTimeout true.
    var IDLE_BUDGET_MS = 50;
    function IdleDeadline() { throw new TypeError('Illegal constructor'); }
    tag(IdleDeadline, 'IdleDeadline');
    function deadline(end, didTimeout) {
        var d = Object.create(IdleDeadline.prototype);
        hide(d, '__end', end);
        hide(d, '__didTimeout', didTimeout);
        return d;
    }
    IdleDeadline.prototype.timeRemaining = function timeRemaining() {
        return this.__didTimeout ? 0 : Math.max(0, this.__end - Date.now());
    };
    Object.defineProperty(IdleDeadline.prototype, 'didTimeout', { get: function () { return this.__didTimeout; }, enumerable: true, configurable: true });
    hide(g, 'IdleDeadline', IdleDeadline);

    var idleId = 0, idleQueue = [], idleTask = null;
    function report(e) {
        var msg;
        try { msg = String(e); } catch (_) { msg = '<unprintable exception>'; }
        if (g.__rustkit_errors) g.__rustkit_errors.push(msg);
    }
    function unqueue(entry) {
        var i = idleQueue.indexOf(entry);
        if (i >= 0) idleQueue.splice(i, 1);
        if (entry.timer !== null) cancel(entry.timer);
        entry.timer = null;
    }
    function invoke(entry, d) {
        unqueue(entry);
        try { entry.cb.call(g, d); } catch (e) { report(e); }
    }
    function timedOut(entry) {
        entry.timer = null;
        if (idleQueue.indexOf(entry) >= 0) invoke(entry, deadline(0, true));
    }
    function scheduleIdle() {
        if (idleTask === null && idleQueue.length) idleTask = post(idlePeriod, 0);
    }
    function idlePeriod() {
        idleTask = null;
        if (typeof g.__rustkit_next_timer === 'function' && g.__rustkit_next_timer() === 0) {
            var now = Date.now();
            idleQueue.slice().forEach(function (entry) {
                if (entry.expires !== null && now >= entry.expires) invoke(entry, deadline(0, true));
            });
            scheduleIdle();
            return;
        }
        var end = Date.now() + IDLE_BUDGET_MS;
        idleQueue.slice().forEach(function (entry) { invoke(entry, deadline(end, false)); });
        scheduleIdle();
    }
    hide(g, 'requestIdleCallback', function requestIdleCallback(callback, options) {
        if (typeof callback !== 'function') {
            throw new TypeError("Failed to execute 'requestIdleCallback' on 'Window': The callback provided as parameter 1 is not a function.");
        }
        var timeout = options && Number(options.timeout) > 0 ? Number(options.timeout) : 0;
        var entry = { id: ++idleId, cb: callback, timer: null, expires: timeout ? Date.now() + timeout : null };
        if (timeout) entry.timer = post(function () { timedOut(entry); }, timeout);
        idleQueue.push(entry);
        scheduleIdle();
        return entry.id;
    });
    hide(g, 'cancelIdleCallback', function cancelIdleCallback(id) {
        for (var i = 0; i < idleQueue.length; i++) {
            if (idleQueue[i].id === id) { unqueue(idleQueue[i]); return; }
        }
    });
})(globalThis);
