// Form-control state that lives in script (HTML §4.10): checkedness,
// select/option selectedness, the button/label/fieldset reflections,
// form.elements/length/requestSubmit and constraint validation, plus the
// `Image` and `Option` named constructors. Evaluated after the Rust-backed
// DOM wrappers (dom.rs), whose text-control value and reset path it keeps:
// it only wraps `form.reset` to also reset the state kept here.
(function (g) {
    // dom.rs's side of checkedness and activation (see there).
    var internals = g.__rkFormInternals || {};
    delete g.__rkFormInternals;
    var HTMLElement = g.HTMLElement;
    if (typeof HTMLElement !== 'function' || typeof g.HTMLFormElement !== 'function') return;
    var Input = g.HTMLInputElement.prototype, Form = g.HTMLFormElement.prototype;
    var Select = g.HTMLSelectElement.prototype, Option = g.HTMLOptionElement.prototype;
    var Button = g.HTMLButtonElement.prototype, Label = g.HTMLLabelElement.prototype;
    var FieldSet = g.HTMLFieldSetElement.prototype, Img = g.HTMLImageElement.prototype;

    function getter(proto, name, fn) {
        Object.defineProperty(proto, name, { get: fn, configurable: true, enumerable: true });
    }
    function accessor(proto, name, get, set) {
        Object.defineProperty(proto, name, { get: get, set: set, configurable: true, enumerable: true });
    }
    function reflectString(proto, name, attr) {
        accessor(proto, name, function () { return this.getAttribute(attr) || ''; },
            function (v) { this.setAttribute(attr, v); });
    }
    function reflectBool(proto, name, attr) {
        accessor(proto, name, function () { return this.hasAttribute(attr); },
            function (v) { this.toggleAttribute(attr, !!v); });
    }
    function descendants(root, tags) {
        return Array.prototype.filter.call(root.getElementsByTagName('*'), function (el) {
            return tags.test(el.localName);
        });
    }
    // A static snapshot, like the other collections here, with the named
    // properties (`elements.email`) pages read.
    function collection(els) {
        var out = Object.create(g.HTMLCollection.prototype);
        els.forEach(function (el, i) { out[i] = el; });
        Object.defineProperty(out, 'length', { value: els.length });
        els.forEach(function (el) {
            [el.id, el.getAttribute('name')].forEach(function (k) {
                if (k && !(k in out)) Object.defineProperty(out, k, { value: el });
            });
        });
        return out;
    }
    if (!g.HTMLCollection.prototype.namedItem) {
        g.HTMLCollection.prototype.namedItem = function (name) {
            name = String(name);
            for (var i = 0; i < this.length; i++) {
                if (name && (this[i].id === name || this[i].getAttribute('name') === name)) return this[i];
            }
            return null;
        };
    }

    // ---- input checkedness (HTML §4.10.5.4): the `checked` attribute until
    // script or the user sets it (the dirty checkedness flag).
    var checks = new WeakMap();
    function radioGroup(el) {
        // With no form owner the group is the radio's tree (HTML
        // §4.10.5.1.16): the document, a shadow tree, or a detached one.
        var name = el.getAttribute('name'), form = el.form;
        var scope = form || el.getRootNode();
        if (!name || !scope || scope === el) return [];
        return Array.prototype.filter.call(scope.querySelectorAll('input'), function (o) {
            return o !== el && o.type === 'radio' && o.getAttribute('name') === name && o.form === form;
        });
    }
    // The engine styles and paints a control from what it is told here.
    var noteChecked = internals.noteChecked || function () {};
    function setChecked(el, v) {
        checks.set(el, v);
        noteChecked(el, v);
    }
    // The checkedness itself. The engine's own changes (a click's
    // activation, a label, a radio group) go through these two, never
    // through the `checked` accessor below: that one is script's, and a
    // page may have wrapped it (React's value tracker does, and takes a
    // write it sees for a state it already knows).
    function isChecked(el) {
        return checks.has(el) ? checks.get(el) : el.hasAttribute('checked');
    }
    function check(el, v) {
        setChecked(el, !!v);
        if (v && el.type === 'radio') radioGroup(el).forEach(function (o) { setChecked(o, false); });
    }
    accessor(Input, 'checked', function () { return isChecked(this); }, function (v) { check(this, v); });

    // ---- activation behaviour of a click (HTML §4.10.5.1.15 checkbox,
    // §4.10.5.1.16 radio button, §4.10.4 label, §4.11.2 summary). dom.rs's
    // dispatch asks before the listeners run and calls the answer after them.
    function checkable(el) {
        return el.localName === 'input' && (el.type === 'checkbox' || el.type === 'radio');
    }
    // The nearest element up from the target with a behaviour here. Other
    // interactive content in between keeps the click to itself.
    var INTERACTIVE = /^(button|select|textarea|input|summary|option)$/;
    function activationTarget(t) {
        for (var n = t; n && n.nodeType === 1; n = n.parentNode) {
            if (n.localName === 'label' || checkable(n) || isSubmitButton(n) || isResetButton(n)) return n;
            if (isDetailsSummary(n)) return n;
            if (INTERACTIVE.test(n.localName) || (n.localName === 'a' && n.hasAttribute('href'))) return null;
        }
        return null;
    }
    // The engine's own events, click and reset. `Event` is taken now:
    // a page that replaces `window.Event` later does not build them, and a
    // page that replaces `click`, `reset` or `dispatchEvent` on a control,
    // a form or a prototype does not stand in for the engine's.
    var EventCtor = g.Event, dispatch = internals.dispatch, clickNow = internals.click, resetText = internals.reset;
    // `input` and `change` after a click's activation: trusted, whoever
    // made the click (Chrome 148: also after script's `click()`).
    // What the engine adds to its own event: an own property, defined
    // with built-ins taken now and a descriptor that inherits nothing.
    var defineOwn = Object.defineProperty, objCreate = Object.create;
    function own(o, k, v, open) {
        var d = objCreate(null);
        d.value = v; d.enumerable = true; d.writable = !!open; d.configurable = !!open;
        defineOwn(o, k, d);
    }
    function fire(el, type, init) {
        var e = __rk.make('Event', type, init || { bubbles: true });
        __rk.trust(e);
        dispatch(el, e);
    }
    if (internals.setActivation) internals.setActivation(function (target, event) {
        var el = activationTarget(target);
        if (!el) return null;
        if (el.localName === 'label') {
            return function (ok) {
                var c = ok ? el.control : null;
                // The labeled control is focused, then clicked (Chrome for
                // Testing 148: `focus` at the control before its `click`).
                // The click is the engine's, trusted when the label's was.
                if (c && !c.hasAttribute('disabled')) {
                    if (typeof c.focus === 'function') c.focus();
                    clickNow(c, !!(event && event.isTrusted));
                }
            };
        }
        if (el.localName === 'summary') {
            return function (ok) {
                var d = el.parentNode;
                if (ok && isDetailsSummary(el)) setOpen(d, !d.hasAttribute('open'));
            };
        }
        if (el.hasAttribute('disabled')) return null;
        if (!checkable(el)) {
            // A submit or reset button acts on its form (HTML §4.10.6).
            return function (ok) {
                var form = ok ? el.closest('form') : null;
                if (!form) return;
                if (isSubmitButton(el)) submit(form, el); else resetAll(form);
            };
        }
        // Legacy-pre-activation: the control is already in its new state
        // when the click's listeners run, and a cancelled click undoes it.
        var was = isChecked(el), radio = el.type === 'radio';
        var before = radio ? radioGroup(el).filter(isChecked)[0] : null;
        check(el, radio ? true : !was);
        return function (ok) {
            if (!ok) {
                check(el, was);
                if (before) check(before, true);
                return;
            }
            if (isChecked(el) === was || !el.isConnected) return;
            fire(el, 'input');
            fire(el, 'change');
        };
    });
    reflectBool(Input, 'defaultChecked', 'checked');
    // No file picker yet: a file input's list is always empty.
    var fileLists = new WeakMap();
    getter(Input, 'files', function () {
        if (this.type !== 'file') return null;
        var f = fileLists.get(this);
        if (!f) {
            f = Object.create(g.FileList ? g.FileList.prototype : Object.prototype, {
                length: { value: 0 },
                item: { value: function () { return null; } },
                [Symbol.iterator]: { value: function () { return [][Symbol.iterator](); } }
            });
            fileLists.set(this, f);
        }
        return f;
    });

    // ---- select and option selectedness (HTML §4.10.7, §4.10.10). An
    // option's selectedness is its `selected` attribute until script sets
    // it (`picked`); the select keeps one selected option when single.
    var picked = new WeakMap();
    function optionsOf(sel) {
        return descendants(sel, /^option$/).filter(function (o) { return o.closest('select') === sel; });
    }
    function selectOf(o) {
        var p = o.parentNode;
        if (p && p.localName === 'optgroup') p = p.parentNode;
        return p && p.localName === 'select' ? p : null;
    }
    function rawSelected(o) { return picked.has(o) ? picked.get(o) : o.hasAttribute('selected'); }
    // The engine styles (`:checked`) and paints a select from what it is
    // told here, through the same note as an input's checkedness.
    function setPicked(o, v) {
        if (picked.has(o) && picked.get(o) === v) return;
        picked.set(o, v);
        noteChecked(o, v);
    }
    // The selectedness setting algorithm: in a single select, the last
    // selected option wins and, with none selected, the first enabled one is.
    // Untouched selects get it on every read; `settle` makes it stick.
    function states(sel, settle) {
        var opts = optionsOf(sel), v = opts.map(rawSelected);
        if (!sel.multiple) {
            var last = v.lastIndexOf(true);
            v = v.map(function (s, i) { return i === last; });
            if (last < 0 && (settle || !opts.some(function (o) { return picked.has(o); }))) {
                var first = opts.findIndex(function (o) { return !o.disabled; });
                if (first >= 0) v[first] = true;
            }
        }
        if (settle) opts.forEach(function (o, i) { setPicked(o, v[i]); });
        return { opts: opts, v: v };
    }
    function selectedOf(sel) {
        var s = states(sel, false);
        return s.opts.filter(function (o, i) { return s.v[i]; });
    }
    function pick(sel, match) {
        optionsOf(sel).forEach(function (o, i) { setPicked(o, match(o, i)); });
    }
    // The user picked option `index` of a single select in its drop-down
    // or with a key (HTML §4.10.7: `input` then `change`, both trusted and
    // bubbling). Nothing fires when it was already the selected one.
    // Returns whether the selection changed.
    if (internals.setPickOption) internals.setPickOption(function (sel, index) {
        var opts = optionsOf(sel), o = opts[index];
        var group = o && o.parentNode && o.parentNode.localName === 'optgroup' ? o.parentNode : null;
        if (!o || o.disabled || (group && group.hasAttribute('disabled'))) return false;
        if (sel.multiple || sel.selectedIndex === index) return false;
        pick(sel, function (x) { return x === o; });
        fire(sel, 'input', { bubbles: true, composed: true });
        fire(sel, 'change');
        return true;
    });
    getter(Select, 'options', function () { return collection(optionsOf(this)); });
    getter(Select, 'length', function () { return optionsOf(this).length; });
    getter(Select, 'selectedOptions', function () { return collection(selectedOf(this)); });
    getter(Select, 'type', function () { return this.multiple ? 'select-multiple' : 'select-one'; });
    accessor(Select, 'selectedIndex', function () {
        var s = states(this, false);
        return s.v.indexOf(true);
    }, function (n) {
        n = Number(n);
        pick(this, function (o, i) { return i === n; });
    });
    accessor(Select, 'value', function () {
        var o = selectedOf(this)[0];
        return o ? o.value : '';
    }, function (v) {
        var found = false;
        v = String(v);
        pick(this, function (o) { return !found && o.value === v && (found = true); });
    });
    Select.add = function (el, before) {
        if (typeof before === 'number') before = optionsOf(this)[before] || null;
        var parent = before && before.parentNode ? before.parentNode : this;
        parent.insertBefore(el, before || null);
        if (!this.multiple && rawSelected(el)) {
            pick(this, function (o) { return o === el; });
        }
        states(this, true);
    };
    getter(Select, 'form', function () { return this.closest('form'); });
    reflectBool(Select, 'multiple', 'multiple');
    reflectBool(Select, 'disabled', 'disabled');
    reflectBool(Select, 'required', 'required');
    reflectString(Select, 'name', 'name');

    accessor(Option, 'selected', function () {
        var sel = selectOf(this);
        if (!sel) return rawSelected(this);
        var s = states(sel, false);
        return s.v[s.opts.indexOf(this)];
    }, function (v) {
        var sel = selectOf(this), self = this;
        setPicked(this, !!v);
        if (!sel) return;
        if (v && !sel.multiple) pick(sel, function (o) { return o === self; });
        states(sel, true);
    });
    reflectBool(Option, 'defaultSelected', 'selected');
    reflectBool(Option, 'disabled', 'disabled');
    // HTML §4.10.10: text strips and collapses ASCII whitespace; value
    // falls back to it.
    accessor(Option, 'text', function () {
        return this.textContent.replace(/[\t\n\f\r ]+/g, ' ').trim();
    }, function (v) { this.textContent = v; });
    accessor(Option, 'value', function () {
        var v = this.getAttribute('value');
        return v === null ? this.text : v;
    }, function (v) { this.setAttribute('value', v); });
    accessor(Option, 'label', function () {
        var v = this.getAttribute('label');
        return v === null ? this.text : v;
    }, function (v) { this.setAttribute('label', v); });
    getter(Option, 'index', function () {
        var sel = selectOf(this);
        return sel ? optionsOf(sel).indexOf(this) : 0;
    });
    getter(Option, 'form', function () {
        var sel = selectOf(this);
        return sel ? sel.form : null;
    });

    // ---- button, label, fieldset (HTML §4.10.6, §4.10.4, §4.10.15)
    accessor(Button, 'type', function () {
        var t = (this.getAttribute('type') || '').toLowerCase();
        return t === 'reset' || t === 'button' ? t : 'submit';
    }, function (v) { this.setAttribute('type', v); });
    reflectBool(Button, 'disabled', 'disabled');
    reflectString(Button, 'name', 'name');
    reflectString(Button, 'value', 'value');
    getter(Button, 'form', function () { return this.closest('form'); });

    var LABELABLE = /^(button|input|meter|output|progress|select|textarea)$/;
    function labelable(el) {
        return !!el && LABELABLE.test(el.localName) && !(el.localName === 'input' && el.type === 'hidden');
    }
    reflectString(Label, 'htmlFor', 'for');
    getter(Label, 'control', function () {
        if (this.hasAttribute('for')) {
            var el = this.isConnected ? this.ownerDocument.getElementById(this.getAttribute('for')) : null;
            return labelable(el) ? el : null;
        }
        return descendants(this, LABELABLE).filter(labelable)[0] || null;
    });
    getter(Label, 'form', function () {
        var c = this.control;
        return c && c.form !== undefined ? c.form : null;
    });

    // Listed elements (HTML §4.10.2).
    var LISTED = /^(button|fieldset|input|object|output|select|textarea)$/;
    reflectBool(FieldSet, 'disabled', 'disabled');
    reflectString(FieldSet, 'name', 'name');
    getter(FieldSet, 'type', function () { return 'fieldset'; });
    getter(FieldSet, 'form', function () { return this.closest('form'); });
    getter(FieldSet, 'elements', function () { return collection(descendants(this, LISTED)); });

    // ---- form (HTML §4.10.3). Image buttons are not in `elements`.
    function listedOf(form) {
        return descendants(form, LISTED).filter(function (el) {
            return !(el.localName === 'input' && el.type === 'image') && el.closest('form') === form;
        });
    }
    getter(Form, 'elements', function () { return collection(listedOf(this)); });
    getter(Form, 'length', function () { return listedOf(this).length; });
    reflectBool(Form, 'noValidate', 'novalidate');
    // Statically validate constraints: every invalid control gets its
    // `invalid` event, so no short-circuit.
    function validate(form) {
        return listedOf(form).reduce(function (ok, el) {
            return (typeof el.checkValidity === 'function' ? el.checkValidity() : true) && ok;
        }, true);
    }
    Form.checkValidity = function () { return validate(this); };
    Form.reportValidity = Form.checkValidity;
    function isSubmitButton(el) {
        if (!el || typeof el.localName !== 'string') return false;
        if (el.localName === 'button') return el.type === 'submit';
        return el.localName === 'input' && (el.type === 'submit' || el.type === 'image');
    }
    function isResetButton(el) {
        return (el.localName === 'button' || el.localName === 'input') && el.type === 'reset';
    }
    // HTML §4.10.21.3 from validation on: the `submit` event, and when no
    // listener cancels it the engine is told. A click's own submit is in
    // its outcome; one made from a timer or a callback waits for the
    // embedder to take it (`Engine::take_script_navigation`).
    var noteSubmit = internals.noteSubmit || function () {};
    function submit(form, submitter) {
        if (!form.isConnected) return;
        var noValidate = form.noValidate || (submitter !== null && submitter.hasAttribute('formnovalidate'));
        if (!noValidate && !validate(form)) return;
        // The engine's event (trusted, as Chrome's is whoever asked for the
        // submission), dispatched as the engine dispatches.
        var e = __rk.make('Event', 'submit', { bubbles: true, cancelable: true });
        own(e, 'submitter', submitter);
        __rk.trust(e);
        if (dispatch(form, e)) noteSubmit(form, submitter);
    }
    // Implicit submission (HTML §4.10.21.2): Enter in a text field clicks
    // the form's default button, its first submit button. A form with none
    // is submitted as it is. A textarea takes the Enter itself.
    if (internals.setImplicitSubmit) internals.setImplicitSubmit(function (el) {
        var form = el.localName === 'input' ? el.closest('form') : null;
        if (!form) return;
        var button = descendants(form, /^(button|input)$/).filter(function (b) {
            return isSubmitButton(b) && b.closest('form') === form;
        })[0];
        if (!button) return submit(form, null);
        if (!button.hasAttribute('disabled')) button.click();
    });
    Form.requestSubmit = function (submitter) {
        if (submitter != null) {
            if (!isSubmitButton(submitter)) {
                throw new TypeError("Failed to execute 'requestSubmit': the submitter is not a submit button.");
            }
            if (submitter.form !== this) {
                throw new g.DOMException('The submitter is not owned by this form.', 'NotFoundError');
            }
        } else {
            submitter = null;
        }
        submit(this, submitter);
    };
    // submit() goes straight to the submission: no validation and no
    // `submit` event (HTML §4.10.3).
    Form.submit = function () {
        if (this.isConnected) noteSubmit(this, null);
    };
    // Reset also restores checkedness and selectedness. The dom.rs reset
    // fires the cancelable `reset` event; its answer decides.
    // `resetAll` is the reset itself: `form.reset()` calls it, and so does
    // a reset button's activation (not through the form's `reset` property).
    function resetAll(form) {
        if (!resetText(form)) return;
        descendants(form, /^(input|select)$/).forEach(function (el) {
            if (el.localName === 'input') {
                if (checks.delete(el)) noteChecked(el, null);
                return;
            }
            optionsOf(el).forEach(function (o) { if (picked.delete(o)) noteChecked(o, null); });
        });
    }
    Form.reset = function reset() { resetAll(this); };

    // ---- details (HTML §4.11.1, §4.11.2): `open` is the attribute; the
    // engine renders a closed details as its summary alone. Only its first
    // summary child opens it. `toggle` follows a change made by a click or
    // through `open`, once the running script is done.
    function isDetailsSummary(n) {
        var p = n.parentNode;
        if (n.localName !== 'summary' || !p || p.localName !== 'details') return false;
        for (var c = p.firstChild; c; c = c.nextSibling) {
            if (c.nodeType === 1 && c.localName === 'summary') return c === n;
        }
        return false;
    }
    function setOpen(details, open) {
        var was = details.hasAttribute('open');
        if (was === open) return;
        details.toggleAttribute('open', open);
        Promise.resolve().then(function () {
            var e = __rk.make('Event', 'toggle');
            own(e, 'oldState', was ? 'open' : 'closed', true);
            own(e, 'newState', open ? 'open' : 'closed', true);
            __rk.trust(e);
            dispatch(details, e);
        });
    }
    if (typeof g.HTMLDetailsElement === 'function') {
        accessor(g.HTMLDetailsElement.prototype, 'open', function () { return this.hasAttribute('open'); },
            function (v) { setOpen(this, !!v); });
        reflectString(g.HTMLDetailsElement.prototype, 'name', 'name');
    }

    // ---- img size (HTML §4.8.4.3): the attribute when there is one, else
    // the image's own size once loaded, else 0. Chrome answers the rendered
    // box, which needs a layout; the same number for an image CSS does not
    // size. Read here without one: a page's load handlers read it for every
    // card after writing a style, and the engine's forced layout of a large
    // page is far dearer than Chrome's (E54, a recorded search page: 22 such
    // reads were 15.7 s).
    ['width', 'height'].forEach(function (k) {
        accessor(Img, k, function () {
            var n = parseInt(this.getAttribute(k), 10);
            if (n >= 0) return n;
            return (k === 'width' ? this.naturalWidth : this.naturalHeight) || 0;
        }, function (v) { this.setAttribute(k, String(v >>> 0)); });
    });
    // img.src: the `src` attribute (whatever loads an img loads this one),
    // read back resolved against the document's URL, '' when absent.
    accessor(Img, 'src', function () {
        var v = this.getAttribute('src');
        if (v === null) return '';
        try { return new g.URL(v, g.location.href).href; } catch (e) { return v; }
    }, function (v) { this.setAttribute('src', v); });

    // ---- named constructors (HTML §4.8.3, §4.10.10). The made elements
    // are ordinary ones: `src` goes through the img element's attribute.
    function Image(width, height) {
        var img = g.document.createElement('img');
        if (width !== undefined) img.width = width;
        if (height !== undefined) img.height = height;
        return img;
    }
    function OptionCtor(text, value, defaultSelected, selected) {
        var o = g.document.createElement('option');
        text = text === undefined ? '' : String(text);
        if (text !== '') o.textContent = text;
        if (value !== undefined) o.value = value;
        if (defaultSelected) o.setAttribute('selected', '');
        picked.set(o, !!selected);
        return o;
    }
    Object.defineProperty(OptionCtor, 'name', { value: 'Option' });
    Image.prototype = Img;
    OptionCtor.prototype = Option;
    [['Image', Image], ['Option', OptionCtor]].forEach(function (p) {
        Object.defineProperty(g, p[0], { value: p[1], writable: true, configurable: true, enumerable: false });
    });
})(globalThis);
