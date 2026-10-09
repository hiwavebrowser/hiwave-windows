// The script-visible CSSOM (CSSOM §6, CSS Conditional §8, CSSOM §9):
// document.styleSheets, <style>.sheet / <link rel=stylesheet>.sheet,
// a constructable CSSStyleSheet, CSSRuleList / CSSStyleRule and the rule's
// style declaration, document.adoptedStyleSheets, CSS.supports and CSS.escape.
//
// Pure JS over the DOM bindings. A <style>'s sheet is parsed from the
// element's text and reparsed when that text changes. insertRule, deleteRule
// and rule edits write back into the <style>'s text, which is what the
// engine's style pipeline reads on the next relayout: that is how CSS-in-JS
// libraries (emotion, styled-components) inject styles.
//
// Stated limits: adoptedStyleSheets and constructed sheets are not applied by
// the engine (it reads only <style>/<link>); `disabled` is recorded but not
// honoured; a <link>'s sheet has no rules (the bindings never see its text);
// at-rules (@media, @font-face, ...) are CSSRule objects with their text only,
// no nested cssRules; CSS.supports checks property names against the list the
// engine applies and validates values only for display and position.
(function (g) {
    var document = g.document;
    if (!document || !g.Document || !g.HTMLStyleElement) return;

    function domError(message, name) { return new g.DOMException(message, name); }
    function illegal() { throw new TypeError('Illegal constructor'); }
    function iface(name, ctor, parent) {
        ctor = ctor || function () { illegal(); };
        Object.defineProperty(ctor, 'name', { value: name });
        if (parent) Object.setPrototypeOf(ctor.prototype, parent.prototype);
        Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true });
        Object.defineProperty(g, name, { value: ctor, writable: true, configurable: true, enumerable: false });
        return ctor;
    }
    function getter(o, name, get, set) {
        Object.defineProperty(o, name, { get: get, set: set, configurable: true, enumerable: true });
    }

    // ---- Lexing: comments out; split at a top-level character, skipping
    // strings and anything nested in (), [] or {}.
    function stripComments(s) {
        var out = '', q = null;
        for (var i = 0; i < s.length; i++) {
            var c = s[i];
            if (q) { out += c; if (c === '\\') out += s[++i] || ''; else if (c === q) q = null; continue; }
            if (c === '"' || c === "'") { q = c; out += c; continue; }
            if (c === '/' && s[i + 1] === '*') { var e = s.indexOf('*/', i + 2); i = e < 0 ? s.length : e + 1; continue; }
            out += c;
        }
        return out;
    }
    function splitTop(s, sep) {
        var parts = [], depth = 0, q = null, start = 0;
        for (var i = 0; i < s.length; i++) {
            var c = s[i];
            if (q) { if (c === '\\') i++; else if (c === q) q = null; continue; }
            if (c === '"' || c === "'") q = c;
            else if (c === '(' || c === '[' || c === '{') depth++;
            else if (c === ')' || c === ']' || c === '}') depth--;
            else if (c === sep && depth === 0) { parts.push(s.slice(start, i)); start = i + 1; }
        }
        parts.push(s.slice(start));
        return parts;
    }
    // Top-level rules as raw text: `prelude { block }` or `@statement;`.
    function splitRules(css) {
        var s = stripComments(css), rules = [], depth = 0, q = null, start = 0;
        for (var i = 0; i < s.length; i++) {
            var c = s[i];
            if (q) { if (c === '\\') i++; else if (c === q) q = null; continue; }
            if (c === '"' || c === "'") q = c;
            else if (c === '{') depth++;
            else if (c === '}' && depth > 0 && --depth === 0) { rules.push(s.slice(start, i + 1)); start = i + 1; }
            else if (c === ';' && depth === 0) { rules.push(s.slice(start, i + 1)); start = i + 1; }
        }
        if (s.slice(start).trim()) rules.push(s.slice(start));
        return rules.map(function (r) { return r.trim(); }).filter(function (r) { return r && r !== ';'; });
    }
    function parseDecls(text) {
        var out = [];
        splitTop(text, ';').forEach(function (d) {
            var i = d.indexOf(':');
            if (i < 0) return;
            var name = d.slice(0, i).trim(), value = d.slice(i + 1).trim(), important = false;
            if (!name) return;
            if (name.slice(0, 2) !== '--') name = name.toLowerCase();
            var m = /\s*!\s*important\s*$/i.exec(value);
            if (m) { important = true; value = value.slice(0, m.index); }
            setDecl(out, name, value, important);
        });
        return out;
    }
    function setDecl(list, name, value, important) {
        for (var i = 0; i < list.length; i++) {
            if (list[i].name === name) { list[i].value = value; list[i].important = important; return; }
        }
        list.push({ name: name, value: value, important: important });
    }

    // ---- CSSStyleDeclaration for a rule (camelCase access through a Proxy,
    // as for element.style).
    var DECLS = Symbol('rustkit.cssom.decls'), RULE = Symbol('rustkit.cssom.rule');
    function cssName(p) { return p === 'cssFloat' ? 'float' : p.slice(0, 2) === '--' ? p : p.replace(/[A-Z]/g, function (c) { return '-' + c.toLowerCase(); }); }
    var Decl = g.CSSStyleDeclaration;
    function RuleStyle() { illegal(); }
    RuleStyle.prototype = Object.create(Decl ? Decl.prototype : Object.prototype);
    function findDecl(st, name) {
        name = String(name);
        if (name.slice(0, 2) !== '--') name = name.toLowerCase();
        var ds = st[DECLS];
        for (var i = 0; i < ds.length; i++) if (ds[i].name === name) return ds[i];
        return null;
    }
    function touched(st) { var r = st[RULE]; r._raw = null; changed(r._sheet); }
    var declMethods = {
        getPropertyValue: function (n) { var d = findDecl(this, n); return d ? d.value : ''; },
        getPropertyPriority: function (n) { var d = findDecl(this, n); return d && d.important ? 'important' : ''; },
        setProperty: function (n, v, prio) {
            n = cssName(String(n));
            if (v == null || String(v) === '') return this.removeProperty(n);
            setDecl(this[DECLS], n.slice(0, 2) === '--' ? n : n.toLowerCase(), String(v).trim(),
                String(prio || '').toLowerCase() === 'important');
            touched(this);
        },
        removeProperty: function (n) {
            var d = findDecl(this, n);
            if (!d) return '';
            this[DECLS].splice(this[DECLS].indexOf(d), 1);
            touched(this);
            return d.value;
        },
        item: function (i) { var ds = this[DECLS]; i = i >>> 0; return i < ds.length ? ds[i].name : ''; }
    };
    Object.keys(declMethods).forEach(function (k) { RuleStyle.prototype[k] = declMethods[k]; });
    getter(RuleStyle.prototype, 'length', function () { return this[DECLS].length; });
    getter(RuleStyle.prototype, 'parentRule', function () { return this[RULE]; });
    getter(RuleStyle.prototype, 'cssText', function () {
        return this[DECLS].map(function (d) {
            return d.name + ': ' + d.value + (d.important ? ' !important' : '') + ';';
        }).join(' ');
    }, function (v) { this[DECLS].length = 0; [].push.apply(this[DECLS], parseDecls(String(v))); touched(this); });
    function makeStyle(rule, decls) {
        var target = Object.create(RuleStyle.prototype), st;
        Object.defineProperty(target, DECLS, { value: decls });
        Object.defineProperty(target, RULE, { value: rule });
        st = new Proxy(target, {
            get: function (t, p) {
                if (typeof p !== 'string' || p in t) return Reflect.get(t, p, st);
                if (/^\d+$/.test(p)) return t.item(Number(p)) || undefined;
                return t.getPropertyValue(cssName(p));
            },
            set: function (t, p, v) {
                if (typeof p !== 'string' || p in t) return Reflect.set(t, p, v, st);
                t.setProperty(cssName(p), v);
                return true;
            }
        });
        return st;
    }

    // ---- Rules
    var CSSRule = iface('CSSRule');
    var CSSStyleRule = iface('CSSStyleRule', null, CSSRule);
    var TYPES = { STYLE_RULE: 1, CHARSET_RULE: 2, IMPORT_RULE: 3, MEDIA_RULE: 4, FONT_FACE_RULE: 5,
                  PAGE_RULE: 6, KEYFRAMES_RULE: 7, KEYFRAME_RULE: 8, NAMESPACE_RULE: 10, SUPPORTS_RULE: 12 };
    Object.keys(TYPES).forEach(function (k) { CSSRule[k] = CSSRule.prototype[k] = TYPES[k]; });
    var AT_TYPES = { charset: 2, 'import': 3, media: 4, 'font-face': 5, page: 6, keyframes: 7, namespace: 10, supports: 12 };
    getter(CSSRule.prototype, 'cssText', function () { return this._raw; });
    getter(CSSRule.prototype, 'type', function () { return this._type; });
    getter(CSSRule.prototype, 'parentStyleSheet', function () { return this._sheet; });
    getter(CSSRule.prototype, 'parentRule', function () { return null; });
    getter(CSSStyleRule.prototype, 'selectorText', function () { return this._selector; }, function (v) {
        v = String(v).trim().replace(/\s+/g, ' ');
        if (!v || /[{};]/.test(v)) return;
        this._selector = v; this._raw = null; changed(this._sheet);
    });
    getter(CSSStyleRule.prototype, 'style', function () { return this._style; }, function (v) { this._style.cssText = v; });
    getter(CSSStyleRule.prototype, 'cssText', function () {
        var body = this._style.cssText;
        return this._selector + ' {' + (body ? ' ' + body + ' ' : ' ') + '}';
    });
    // The text a rule writes back to its <style>: its source until edited.
    function sourceOf(r) { return r._raw === null ? r.cssText : r._src; }

    // One raw rule -> a rule object, or null when it is not a rule.
    function parseRule(raw, sheet) {
        var r;
        if (raw[0] === '@') {
            var name = (/^@([-\w]+)/.exec(raw) || [])[1];
            if (!name) return null;
            name = name.toLowerCase().replace(/^-\w+-(?=keyframes)/, '');
            r = Object.create(CSSRule.prototype);
            r._type = AT_TYPES[name] || 0;
            r._raw = raw;
        } else {
            var open = raw.indexOf('{');
            if (open <= 0 || raw[raw.length - 1] !== '}') return null;
            var sel = raw.slice(0, open).trim().replace(/\s+/g, ' ');
            if (!sel) return null;
            r = Object.create(CSSStyleRule.prototype);
            r._type = 1;
            r._selector = sel;
            r._raw = undefined;
            r._style = makeStyle(r, parseDecls(raw.slice(open + 1, -1)));
        }
        r._src = raw;
        r._sheet = sheet;
        return r;
    }
    function parseSheet(text, sheet, skipImport) {
        var out = [];
        splitRules(text).forEach(function (raw) {
            var r = parseRule(raw, sheet);
            if (r && !(skipImport && r._type === 3)) out.push(r);
        });
        return out;
    }

    // ---- Lists
    var CSSRuleList = iface('CSSRuleList');
    var StyleSheetList = iface('StyleSheetList');
    [CSSRuleList, StyleSheetList].forEach(function (C) {
        C.prototype.item = function (i) { return this[i >>> 0] || null; };
        C.prototype[Symbol.iterator] = Array.prototype.values;
    });
    function fill(list, items) {
        for (var i = 0; i < Math.max(list.length || 0, items.length); i++) {
            if (i < items.length) Object.defineProperty(list, i, { value: items[i], configurable: true, enumerable: true });
            else delete list[i];
        }
        Object.defineProperty(list, 'length', { value: items.length, configurable: true });
        return list;
    }

    // ---- Sheets
    var StyleSheet = g.StyleSheet || iface('StyleSheet');
    var constructing = false;
    var CSSStyleSheet = iface('CSSStyleSheet', function CSSStyleSheet(options) {
        if (!(this instanceof CSSStyleSheet)) throw new TypeError("Constructor CSSStyleSheet requires 'new'");
        init(this, null, true);
        options = options || {};
        this._media = options.media == null ? '' : String(options.media);
        this.disabled = !!options.disabled;
    }, StyleSheet);
    function init(sheet, owner, constructed) {
        sheet._owner = owner;
        sheet._constructed = constructed;
        sheet._rules = [];
        sheet._text = '';
        sheet._list = Object.create(CSSRuleList.prototype);
        fill(sheet._list, []);
        sheet.disabled = false;
        return sheet;
    }
    function sheetFor(el) {
        var s = Object.create(CSSStyleSheet.prototype);
        init(s, el, false);
        s._text = null;
        return s;
    }
    function isStyle(o) { return o && o._owner && o._owner.localName === 'style'; }
    // A <style>'s rules follow its text: reparse when script changed it.
    function sync(sheet) {
        if (isStyle(sheet)) {
            var text = sheet._owner.textContent || '';
            if (text !== sheet._text) { sheet._text = text; sheet._rules = parseSheet(text, sheet, false); }
        }
        fill(sheet._list, sheet._rules);
        return sheet;
    }
    // After a rule edit: write the rules back into the <style>'s text.
    function changed(sheet) {
        if (!sheet) return;
        if (isStyle(sheet)) write(sheet, sheet._rules.map(sourceOf).join('\n'));
        fill(sheet._list, sheet._rules);
    }
    function write(sheet, text) { sheet._text = text; sheet._owner.textContent = text; }
    function checkIndex(sheet, index, max) {
        if (index > max) throw domError("Failed to execute on 'CSSStyleSheet': The index provided (" + index +
            ') is larger than the maximum index (' + max + ').', 'IndexSizeError');
    }
    var sheetMethods = {
        insertRule: function (rule, index) {
            sync(this);
            index = index === undefined ? 0 : index >>> 0;
            checkIndex(this, index, this._rules.length);
            var raws = splitRules(String(rule)), r = raws.length === 1 ? parseRule(raws[0], this) : null;
            if (!r) throw domError("Failed to parse the rule '" + rule + "'.", 'SyntaxError');
            if (r._type === 3 && this._constructed) throw domError("Can't insert @import rules into a constructed stylesheet.", 'SyntaxError');
            var atEnd = index === this._rules.length;
            this._rules.splice(index, 0, r);
            if (isStyle(this) && atEnd) {
                // The CSS-in-JS path: append the rule to the element's text.
                write(this, this._text + (this._text ? '\n' : '') + r._src);
                fill(this._list, this._rules);
            } else {
                changed(this);
            }
            return index;
        },
        deleteRule: function (index) {
            sync(this);
            index = index >>> 0;
            checkIndex(this, index, this._rules.length - 1);
            this._rules.splice(index, 1)[0]._sheet = null;
            changed(this);
        },
        replaceSync: function (text) {
            if (!this._constructed) throw domError("Can't call replaceSync on non-constructed CSSStyleSheets.", 'NotAllowedError');
            this._rules = parseSheet(String(text), this, true);
            fill(this._list, this._rules);
        },
        replace: function (text) {
            try { this.replaceSync(text); } catch (e) { return Promise.reject(e); }
            return Promise.resolve(this);
        }
    };
    Object.keys(sheetMethods).forEach(function (k) { CSSStyleSheet.prototype[k] = sheetMethods[k]; });
    getter(CSSStyleSheet.prototype, 'cssRules', function () { return sync(this)._list; });
    getter(CSSStyleSheet.prototype, 'rules', function () { return sync(this)._list; });
    getter(CSSStyleSheet.prototype, 'ownerRule', function () { return null; });
    getter(StyleSheet.prototype, 'type', function () { return 'text/css'; });
    getter(StyleSheet.prototype, 'ownerNode', function () { return this._owner || null; });
    getter(StyleSheet.prototype, 'parentStyleSheet', function () { return null; });
    getter(StyleSheet.prototype, 'href', function () {
        var o = this._owner;
        if (!o || o.localName !== 'link') return null;
        try { return String(new g.URL(o.getAttribute('href') || '', g.location.href)); } catch (e) { return o.getAttribute('href'); }
    });
    getter(StyleSheet.prototype, 'title', function () { return this._owner ? this._owner.getAttribute('title') : null; });
    getter(StyleSheet.prototype, 'media', function () {
        var text = this._owner ? this._owner.getAttribute('media') || '' : this._media || '';
        return { mediaText: text, length: text ? text.split(',').length : 0, toString: function () { return text; } };
    });

    // ---- <style>.sheet, <link rel=stylesheet>.sheet, document.styleSheets
    var sheets = new WeakMap();
    function isSheetLink(el) {
        return el.localName === 'link' && el.hasAttribute('href') &&
            /(^|\s)stylesheet(\s|$)/i.test(el.getAttribute('rel') || '');
    }
    function ownSheet(el) {
        if (!el.isConnected || (el.localName === 'link' && !isSheetLink(el))) return null;
        var s = sheets.get(el);
        if (!s) { s = sheetFor(el); sheets.set(el, s); }
        return s;
    }
    getter(g.HTMLStyleElement.prototype, 'sheet', function () { return ownSheet(this); });
    if (g.HTMLLinkElement) getter(g.HTMLLinkElement.prototype, 'sheet', function () { return ownSheet(this); });
    getter(g.Document.prototype, 'styleSheets', function () {
        var els = [].slice.call(this.getElementsByTagName('style'));
        var links = [].filter.call(this.getElementsByTagName('link'), isSheetLink);
        if (links.length) {
            els = els.concat(links).sort(function (a, b) {
                return a.compareDocumentPosition(b) & 4 ? -1 : 1;
            });
        }
        return fill(Object.create(StyleSheetList.prototype), els.map(ownSheet).filter(Boolean));
    });
    var adopted = new WeakMap();
    getter(g.Document.prototype, 'adoptedStyleSheets', function () {
        var a = adopted.get(this);
        if (!a) { a = []; adopted.set(this, a); }
        return a;
    }, function (v) {
        if (v == null || typeof v[Symbol.iterator] !== 'function') throw new TypeError('adoptedStyleSheets must be an array');
        var list = Array.from(v);
        list.forEach(function (s) {
            if (!(s instanceof CSSStyleSheet)) throw new TypeError("Failed to set 'adoptedStyleSheets': element is not a CSSStyleSheet.");
            if (!s._constructed) throw domError("Can't adopt non-constructed stylesheets.", 'NotAllowedError');
        });
        adopted.set(this, list);
    });

    // ---- CSS namespace: supports() and escape()
    // The properties rustkit-engine's apply_style_property has an arm for,
    // plus the logical properties logical_to_physical maps onto them.
    var PROPS = {};
    ('-webkit-background-clip -webkit-text-fill-color align-content align-items align-self animation animation-delay ' +
     'animation-direction animation-duration animation-fill-mode animation-iteration-count animation-name ' +
     'animation-play-state animation-timing-function aspect-ratio background background-clip background-color ' +
     'background-image background-origin background-position background-repeat background-size border border-bottom ' +
     'border-bottom-color border-bottom-left-radius border-bottom-right-radius border-bottom-style border-bottom-width ' +
     'border-color border-end-end-radius border-end-start-radius border-left border-left-color border-left-style ' +
     'border-left-width border-radius border-right border-right-color border-right-style border-right-width ' +
     'border-start-end-radius border-start-start-radius border-style border-top border-top-color border-top-left-radius ' +
     'border-top-right-radius border-top-style border-top-width border-width bottom box-shadow box-sizing clear color ' +
     'column-count column-gap content display flex flex-basis flex-direction flex-grow flex-shrink flex-wrap float font ' +
     'font-family font-size font-style font-weight gap grid-area grid-auto-columns grid-auto-flow grid-auto-rows ' +
     'grid-column grid-column-end grid-column-start grid-gap grid-row grid-row-end grid-row-start grid-template ' +
     'grid-template-areas grid-template-columns grid-template-rows height inset justify-content justify-items ' +
     'justify-self left letter-spacing line-break line-height margin margin-bottom margin-left margin-right margin-top ' +
     'max-height max-width min-height min-width object-fit opacity order overflow overflow-wrap overflow-x overflow-y ' +
     'padding padding-bottom padding-left padding-right padding-top position right rotate row-gap scale text-align ' +
     'text-decoration text-decoration-color text-decoration-line text-decoration-style text-overflow text-transform top ' +
     'transform transform-origin transition transition-delay transition-duration transition-property ' +
     'transition-timing-function translate vertical-align visibility white-space width word-break word-spacing ' +
     'word-wrap z-index').split(' ').forEach(function (p) { PROPS[p] = true; });
    ['border-inline', 'border-block'].forEach(function (b) {
        ['', '-start', '-end'].forEach(function (side) {
            ['', '-width', '-style', '-color'].forEach(function (part) { PROPS[b + side + part] = true; });
        });
    });
    ['margin', 'padding', 'inset'].forEach(function (b) {
        ['-inline', '-block'].forEach(function (axis) {
            ['', '-start', '-end'].forEach(function (side) { PROPS[b + axis + side] = true; });
        });
    });
    // Mirrors rustkit-css parse_display.
    function displayOk(v) {
        if (/^(block|inline|inline-block|flex|inline-flex|grid|inline-grid|none)$/.test(v)) return true;
        var outer = 0, inner = '', item = false, parts = v.split(/\s+/);
        for (var i = 0; i < parts.length; i++) {
            var t = parts[i];
            if ((t === 'block' || t === 'inline') && !outer) outer = 1;
            else if (/^(flow|flow-root|flex|grid)$/.test(t) && !inner) inner = t;
            else if (t === 'list-item' && !item) item = true;
            else return false;
        }
        return !item || !inner || inner === 'flow' || inner === 'flow-root';
    }
    var VALUES = {
        display: displayOk,
        position: function (v) { return /^(static|relative|absolute|fixed|sticky)$/.test(v); }
    };
    function balanced(v) {
        var depth = 0, q = null;
        for (var i = 0; i < v.length; i++) {
            var c = v[i];
            if (q) { if (c === '\\') i++; else if (c === q) q = null; continue; }
            if (c === '"' || c === "'") q = c;
            else if (c === '(' || c === '[') depth++;
            else if (c === ')' || c === ']') { if (--depth < 0) return false; }
            else if (c === ';' || c === '{' || c === '}' || c === '!') return false;
        }
        return depth === 0 && !q;
    }
    function supportsDecl(prop, value) {
        prop = String(prop).trim();
        value = String(value).trim();
        if (prop.slice(0, 2) === '--') return prop.length > 2 && balanced(value);
        prop = prop.toLowerCase();
        if (!PROPS[prop] || !value || !balanced(value)) return false;
        value = value.toLowerCase();
        if (/^(inherit|initial|unset|revert|revert-layer)$/.test(value)) return true;
        return VALUES[prop] ? VALUES[prop](value) : true;
    }
    // <supports-condition>: not X | X (and X)* | X (or X)*, X in parentheses
    // or selector(...). Answers null when the text does not parse.
    function condition(s) {
        s = s.trim();
        var m = /^not\s+([\s\S]*)$/i.exec(s);
        if (m) { var v = inParens(m[1].trim()); return v === null ? null : !v; }
        var parts = [], ops = [], depth = 0, start = 0, q = null;
        for (var i = 0; i < s.length; i++) {
            var c = s[i];
            if (q) { if (c === '\\') i++; else if (c === q) q = null; continue; }
            if (c === '"' || c === "'") q = c;
            else if (c === '(') depth++;
            else if (c === ')') depth--;
            else if (depth === 0 && /\s/.test(c)) {
                var w = /^\s+(and|or)\s+/i.exec(s.slice(i));
                if (w) { parts.push(s.slice(start, i)); ops.push(w[1].toLowerCase()); i += w[0].length - 1; start = i + 1; }
            }
        }
        parts.push(s.slice(start));
        if (ops.some(function (o) { return o !== ops[0]; })) return null;
        var vals = parts.map(function (p) { return inParens(p.trim()); });
        if (vals.indexOf(null) >= 0) return null;
        return ops[0] === 'or' ? vals.indexOf(true) >= 0 : vals.indexOf(false) < 0;
    }
    function inParens(s) {
        var m = /^selector\(([\s\S]*)\)$/i.exec(s);
        if (m) {
            if (!m[1].trim()) return null;
            try { document.querySelector(m[1]); return true; } catch (e) { return false; }
        }
        if (s[0] !== '(' || s[s.length - 1] !== ')' || !balanced(s.replace(/!/g, ''))) return null;
        var inner = s.slice(1, -1).trim();
        if (/^(\(|not\s)/i.test(inner)) return condition(inner);
        var colon = inner.indexOf(':');
        if (colon > 0 && /^-*[a-zA-Z][-\w]*$/.test(inner.slice(0, colon).trim())) {
            return supportsDecl(inner.slice(0, colon), inner.slice(colon + 1));
        }
        return false;   // <general-enclosed>: valid, but not supported
    }
    function supports(a, b) {
        if (arguments.length >= 2) return supportsDecl(a, b);
        var s = String(a), v = condition(s);
        if (v === null) v = condition('(' + s + ')');
        return v === true;
    }
    // CSSOM §2.1 serialize an identifier.
    function escape(value) {
        var s = String(value), out = '';
        for (var i = 0; i < s.length; i++) {
            var c = s.charCodeAt(i), ch = s[i];
            if (c === 0) out += '�';
            else if ((c >= 1 && c <= 0x1f) || c === 0x7f ||
                     (c >= 0x30 && c <= 0x39 && (i === 0 || (i === 1 && s.charCodeAt(0) === 0x2d)))) {
                out += '\\' + c.toString(16) + ' ';
            } else if (i === 0 && c === 0x2d && s.length === 1) out += '\\' + ch;
            else if (c >= 0x80 || c === 0x2d || c === 0x5f || (c >= 0x30 && c <= 0x39) ||
                     (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a)) out += ch;
            else out += '\\' + ch;
        }
        return out;
    }
    if (!g.CSS) {
        var CSS = {};
        Object.defineProperty(CSS, Symbol.toStringTag, { value: 'CSS' });
        Object.defineProperty(g, 'CSS', { value: CSS, writable: true, configurable: true, enumerable: false });
    }
    g.CSS.supports = supports;
    g.CSS.escape = escape;
})(globalThis);
