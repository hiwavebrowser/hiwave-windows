// Named access on the document's collections: what the pinned Chrome answers
// (named_collections_cft148.json). Each step records one string; a step that
// throws records the error's name. `window.__out` is the JSON of all of them.
(function () {
  var out = {};
  function id(el) {
    if (el === undefined) return 'undefined';
    if (el === null) return 'null';
    if (typeof el !== 'object') return typeof el + ':' + String(el);
    if (el.nodeType === 1) return '#' + el.id;
    return Object.prototype.toString.call(el);
  }
  function step(name, fn) {
    var r;
    try { r = fn(); } catch (e) { r = 'throws ' + (e && e.name); }
    out[name] = typeof r === 'string' ? r : JSON.stringify(r);
  }
  function desc(o, k) {
    var d = Object.getOwnPropertyDescriptor(o, k);
    if (!d) return 'none';
    return ['value' in d ? id(d.value) : 'accessor', 'w' + +!!d.writable, 'e' + +!!d.enumerable, 'c' + +!!d.configurable].join(' ');
  }
  function forIn(o) { var ks = []; for (var k in o) ks.push(k); return ks.join(' '); }
  var F = function () { return document.forms; };

  // ---- shape
  step('forms.tag', function () { return Object.prototype.toString.call(F()) + ' ' + F().length; });
  step('forms.sameObject', function () { return String(F() === F()); });

  // ---- by id, by name
  step('forms[id]', function () { return id(F()['fid']); });
  step('forms[name]', function () { return id(F()['fname']); });
  step('forms.id', function () { return id(F().fid); });
  step('forms.name', function () { return id(F().fname); });
  step('forms.missing', function () { return id(F().nope); });
  step('forms.namedItem(id)', function () { return id(F().namedItem('fid')); });
  step('forms.namedItem(name)', function () { return id(F().namedItem('fname')); });
  step('forms.namedItem(missing)', function () { return id(F().namedItem('nope')); });
  step('forms.namedItem(empty)', function () { return id(F().namedItem('')); });
  step('forms[empty]', function () { return id(F()['']); });

  // ---- precedence: #x has name y, #y has name x; two forms named dup
  step('forms.x', function () { return id(F().x); });
  step('forms.y', function () { return id(F().y); });
  step('forms.dup', function () { return id(F().dup); });
  step('forms.namedItem(dup)', function () { return id(F().namedItem('dup')); });

  // ---- names that are members of the interface or of Object.prototype
  step('forms.length', function () { return typeof F().length + ':' + F().length; });
  step('forms.item', function () { return typeof F().item; });
  step('forms.namedItem', function () { return typeof F().namedItem; });
  step('forms.toString', function () { return typeof F().toString; });
  step('forms.namedItem(length)', function () { return id(F().namedItem('length')); });
  step('forms.namedItem(item)', function () { return id(F().namedItem('item')); });
  step('forms.namedItem(toString)', function () { return id(F().namedItem('toString')); });
  step('own(length)', function () { return desc(F(), 'length'); });
  step('own(item)', function () { return desc(F(), 'item'); });
  step('own(toString)', function () { return desc(F(), 'toString'); });

  // ---- ids that are names of Object.prototype, and `then`
  step('forms.__proto__', function () { return String(F().__proto__ === HTMLCollection.prototype) + ' ' + id(F().namedItem('__proto__')); });
  step('forms.constructor', function () { return String(F().constructor === HTMLCollection) + ' ' + id(F().namedItem('constructor')); });
  step('forms.hasOwnProperty', function () { return typeof F().hasOwnProperty + ' ' + id(F().namedItem('hasOwnProperty')); });
  step('forms.then', function () { return id(F().then) + ' ' + id(F().namedItem('then')); });
  step('own(then)', function () { return desc(F(), 'then').replace(/ c[01]$/, ''); });
  step('own(__proto__)', function () { return desc(F(), '__proto__') + ' ' + desc(F(), 'constructor') + ' ' + desc(F(), 'hasOwnProperty'); });

  // ---- names that look like numbers
  step('forms["0"]', function () { return id(F()['0']); });
  step('forms["2"]', function () { return id(F()['2']); });
  step('forms["40"]', function () { return id(F()['40']); });
  step('forms["01"]', function () { return id(F()['01']); });
  step('forms["-1"]', function () { return id(F()['-1']); });
  step('forms["1e3"]', function () { return id(F()['1e3']); });
  step('forms.namedItem("40")', function () { return id(F().namedItem('40')); });
  step('forms.item("fid")', function () { return id(F().item('fid')); });

  // ---- in, hasOwnProperty, enumeration, descriptors
  step('in', function () {
    var f = F();
    return ['fid', 'fname', 'nope', 'length', 'item', '0', '99', '40', 'dup', ''].map(function (k) { return k + ':' + +(k in f); }).join(' ');
  });
  step('hasOwn', function () {
    var f = F(), h = Object.prototype.hasOwnProperty;
    return ['fid', 'fname', 'nope', 'dup', 'item', '0', '99'].map(function (k) { return k + ':' + +h.call(f, k); }).join(' ');
  });
  step('keys', function () { return Object.keys(F()).join(' '); });
  step('forIn', function () { return forIn(F()); });
  step('forIn.named', function () { return forIn(F()).split(' ').filter(function (k) { return k === 'fid' || k === 'fname' || k === 'dup'; }).join(' '); });
  step('ownNames', function () { return Object.getOwnPropertyNames(F()).join(' '); });
  step('own(fid)', function () { return desc(F(), 'fid'); });
  step('own(fname)', function () { return desc(F(), 'fname'); });
  step('own(0)', function () { return desc(F(), '0'); });
  step('own(nope)', function () { return desc(F(), 'nope'); });
  step('iterate', function () { var n = 0; for (var f of F()) n++; return String(n); });

  // ---- writes
  step('assign.sloppy', function () { var f = F(); f.fid = 1; return id(f.fid); });
  step('assign.strict', function () { 'use strict'; var f = F(); f.fid = 1; return id(f.fid); });
  step('delete.sloppy', function () { var f = F(); var r = delete f.fid; return r + ' ' + id(f.fid); });
  step('delete.strict', function () { 'use strict'; var f = F(); var r = delete f.fid; return r + ' ' + id(f.fid); });
  step('define', function () { var f = F(); Object.defineProperty(f, 'fid', { value: 1 }); return id(f.fid); });
  step('expando', function () { var f = F(); f.mine = 5; return id(f.mine) + ' ' + id(F().mine); });

  // ---- liveness: a collection held from before (`held`), and one read
  // again after each change (`fresh`)
  (function () {
    var held = F(), n0 = held.length, h = [], r = [], f;
    function both(name, hv, rv) {
      var a, b;
      try { a = hv(); } catch (e) { a = 'throws ' + e.name; }
      try { b = rv(); } catch (e) { b = 'throws ' + e.name; }
      h.push(name + ' ' + a); r.push(name + ' ' + b);
    }
    try {
      f = document.createElement('form');
      f.setAttribute('name', 'added');
      document.body.appendChild(f);
      both('add', function () { return [held.length - n0, held.added === f, held[held.length - 1] === f].join(' '); },
                  function () { var c = F(); return [c.length - n0, c.added === f, c[c.length - 1] === f].join(' '); });
      f.setAttribute('name', 'renamed');
      both('rename', function () { return id(held.added) + ' ' + (held.renamed === f); },
                     function () { return id(F().added) + ' ' + (F().renamed === f); });
      f.id = 'lateid';
      both('id', function () { return String(held.lateid === f); }, function () { return String(F().lateid === f); });
      f.removeAttribute('name');
      both('unname', function () { return id(held.renamed); }, function () { return id(F().renamed); });
      document.body.removeChild(f);
      both('remove', function () { return (held.length - n0) + ' ' + id(held.lateid); },
                     function () { return (F().length - n0) + ' ' + id(F().lateid); });
    } catch (e) { h.push('throws ' + e.name); r.push('throws ' + e.name); }
    out['live.held'] = h.join(' | ');
    out['live.fresh'] = r.join(' | ');
  })();

  // ---- the other collections of the document
  step('images', function () { var c = document.images; return [Object.prototype.toString.call(c), c.length, id(c.pid), id(c.pic), id(c['pid2']), id(c.namedItem('pic')), id(c.fid)].join(' '); });
  step('links', function () { var c = document.links; return [Object.prototype.toString.call(c), c.length, id(c.aid), id(c.anc), id(c.arid), id(c.arname), id(c.noh), id(c.nohref)].join(' '); });
  step('anchors', function () { var c = document.anchors; return [Object.prototype.toString.call(c), c.length, id(c.aid), id(c.anc), id(c.noh), id(c.nohref), id(c.arname)].join(' '); });
  step('scripts', function () { var c = document.scripts; return [Object.prototype.toString.call(c), c.length, id(c.sid)].join(' '); });
  step('embeds', function () { var c = document.embeds; return [Object.prototype.toString.call(c), c.length, id(c.eid), id(c.ename)].join(' '); });
  step('plugins', function () { var c = document.plugins; return [Object.prototype.toString.call(c), c.length, id(c.eid), id(c.ename)].join(' '); });
  step('plugins.sameAsEmbeds', function () { return String(document.plugins === document.embeds); });
  step('getElementsByName', function () { var c = document.getElementsByName('pic'); return [Object.prototype.toString.call(c), c.length, id(c.pid), id(c.pic), id(c[0])].join(' '); });
  step('getElementsByTagName', function () { var c = document.getElementsByTagName('form'); return [Object.prototype.toString.call(c), id(c.fid), id(c.fname), id(c.nope), Object.keys(c).length === c.length].join(' '); });
  step('getElementsByClassName', function () { var c = document.getElementsByClassName('c'); return [Object.prototype.toString.call(c), c.length, id(c.k1), id(c.kn), id(c.k2), id(c.svid), id(c.svname)].join(' '); });
  step('children', function () { var c = document.getElementById('kids').children; return [Object.prototype.toString.call(c), c.length, id(c.k1), id(c.kn), id(c.k2), id(c.svid), id(c.svname), id(c.namedItem('svname'))].join(' '); });
  step('form.elements', function () { var c = document.getElementById('ctl').elements; return [Object.prototype.toString.call(c), c.length, id(c.warm), id(c.len), Object.prototype.toString.call(c.grp)].join(' '); });
  step('document[name]', function () { return [id(document.fname), id(document.fid), id(document.pic && document.pic[0]), id(document.pid)].join(' '); });

  window.__out = JSON.stringify(out);
})();
