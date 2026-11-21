//! Incremental restyle PR 1: the invalidation index's classification, and
//! the dirty-set superset test (the design note's §4 Test 1, invariant I2):
//! for each fixture page and mutation, build in full, apply the mutation
//! through the bindings, build in full again, and check that every element
//! whose computed style changed is covered by the predicted `DirtySet`.

use super::*;
use crate::restyle_invalidation::{
    DirtySet, InvalidationIndex, ANCESTOR, ANYWHERE, BOX, IN_HAS, LAYOUT, PAINT, POSITIONAL, SIBLING, STYLE,
    SUBJECT, SUBTREE_STYLE,
};

fn index(selectors: &[&str]) -> InvalidationIndex {
    let mut ix = InvalidationIndex::default();
    for s in selectors {
        // As the sheet parser hands selectors over: escapes resolved.
        ix.note_selector(&rustkit_css::encode_selector_escapes(s));
    }
    ix
}

fn flags(map: &HashMap<String, u8>, key: &str) -> u8 {
    map.get(key).copied().unwrap_or(0)
}

#[test]
fn index_places_tokens_by_combinator() {
    let ix = index(&[".a .b > .c", "#top + .d ~ .e", "[data-open] .x[data-k=v]", ".subj"]);
    assert_eq!(flags(&ix.classes, "a"), ANCESTOR);
    assert_eq!(flags(&ix.classes, "b"), ANCESTOR);
    assert_eq!(flags(&ix.classes, "c"), SUBJECT);
    assert_eq!(flags(&ix.ids, "top"), SIBLING | POSITIONAL);
    assert_eq!(flags(&ix.classes, "d"), SIBLING | POSITIONAL);
    assert_eq!(flags(&ix.classes, "e"), SUBJECT | POSITIONAL);
    assert_eq!(flags(&ix.attributes, "data-open"), ANCESTOR);
    assert_eq!(flags(&ix.attributes, "data-k"), SUBJECT);
    assert_eq!(flags(&ix.classes, "subj"), SUBJECT);
    assert!(ix.positional, "a sibling combinator reads position");
    assert!(!ix.has);
    assert_eq!(flags(&ix.classes, "absent"), 0);
}

#[test]
fn index_marks_structural_rules_positional() {
    let ix = index(&["li.row:nth-child(odd)", ".p:empty", ".q"]);
    assert_eq!(flags(&ix.classes, "row"), SUBJECT | POSITIONAL);
    assert_eq!(flags(&ix.pseudos, "nth-child"), SUBJECT | POSITIONAL);
    assert_eq!(flags(&ix.pseudos, "empty"), SUBJECT | POSITIONAL);
    assert_eq!(flags(&ix.classes, "q"), SUBJECT);
    assert!(ix.positional);
    assert!(!index(&[".q", "#r .s"]).positional);
}

#[test]
fn index_reads_into_functional_pseudo_classes() {
    let ix = index(&[
        ".a:not(.b)",
        ":is(.c, .d) .e",
        ".f:not(.g .h)",
        ".i:has(> .j)",
        ":nth-child(2 of .k)",
        ":host(.l)",
        ".m::slotted(.n)",
        "input:checked + label",
    ]);
    assert_eq!(flags(&ix.classes, "b"), SUBJECT, "a compound in :not() is the element's own");
    assert_eq!(flags(&ix.classes, "c"), ANCESTOR);
    assert_eq!(flags(&ix.classes, "d"), ANCESTOR);
    assert_eq!(flags(&ix.classes, "e"), SUBJECT);
    assert_eq!(flags(&ix.classes, "g"), ANCESTOR, "a complex member of the subject's list places as a rule");
    assert_eq!(flags(&ix.classes, "h"), SUBJECT);
    assert_eq!(flags(&ix.classes, "i"), SUBJECT);
    assert_eq!(flags(&ix.classes, "j"), IN_HAS, ":has() reaches the subjects above (LAT6)");
    assert!(ix.has);
    assert_eq!(ix.has_subjects, vec![(".i".to_string(), SUBJECT)]);
    assert!(!ix.has_anywhere);
    assert_eq!(flags(&ix.classes, "k") & ANYWHERE, ANYWHERE);
    assert_eq!(flags(&ix.classes, "l") & ANYWHERE, ANYWHERE);
    assert_eq!(flags(&ix.classes, "n") & ANYWHERE, ANYWHERE);
    assert_eq!(flags(&ix.classes, "m"), SUBJECT);
    assert_eq!(flags(&ix.pseudos, "checked"), SIBLING | POSITIONAL);
}

#[test]
fn index_places_complex_list_members_by_their_own_combinators() {
    let ix = index(&[".dark\\:x:is(.dark *)", ".t:where(.u + .v)", ":is(.w .x) .y", "li:nth-child(odd):is(.m .n)"]);
    assert_eq!(flags(&ix.classes, "dark"), ANCESTOR, "{:?}", ix.classes);
    assert_eq!(flags(&ix.classes, "dark:x"), SUBJECT);
    assert_eq!(flags(&ix.classes, "u"), SIBLING | POSITIONAL);
    assert_eq!(flags(&ix.classes, "v"), SUBJECT | POSITIONAL);
    assert_eq!(flags(&ix.classes, "w") & ANYWHERE, ANYWHERE, "left of a combinator: unplaced");
    assert_eq!(flags(&ix.classes, "x") & ANYWHERE, ANYWHERE);
    assert_eq!(flags(&ix.classes, "y"), SUBJECT);
    assert_eq!(flags(&ix.classes, "m"), ANCESTOR | POSITIONAL, "the outer rule's position read carries in");
    assert_eq!(flags(&ix.classes, "n"), SUBJECT | POSITIONAL);
}

#[test]
fn index_decodes_escapes_and_attribute_names() {
    let ix = index(&[".md\\:flex", "[DATA-Mode~=\"a]b\"] .y", "a[href^='#'] .z"]);
    assert_eq!(flags(&ix.classes, "md:flex"), SUBJECT, "{:?}", ix.classes);
    assert_eq!(flags(&ix.classes, "y"), SUBJECT, "a bracket in a quoted value is no bracket");
    assert_eq!(flags(&ix.attributes, "data-mode"), ANCESTOR);
    assert_eq!(flags(&ix.attributes, "href"), ANCESTOR);
    assert_eq!(flags(&ix.classes, "z"), SUBJECT);
    assert!(ix.ids.is_empty(), "a '#' in an attribute value names no id: {:?}", ix.ids);
}

#[test]
fn index_places_unparsed_selectors_anywhere() {
    let ix = index(&[".a:not(.b", "& .c"]);
    assert_eq!(flags(&ix.classes, "a") & ANYWHERE, ANYWHERE);
    assert_eq!(flags(&ix.classes, "c") & ANYWHERE, ANYWHERE);
}

/// Each element's computed style in the view's laid-out tree (its first
/// box), by raw NodeId.
fn styles(engine: &Engine, view: EngineViewId) -> HashMap<usize, ComputedStyle> {
    fn walk(b: &LayoutBox, document: &Document, out: &mut HashMap<usize, ComputedStyle>) {
        if let Some(raw) = b.node_id {
            let element = document.get_node(rustkit_dom::NodeId::new(raw)).is_some_and(|n| n.is_element());
            if element {
                out.entry(raw).or_insert_with(|| (*b.style).clone());
            }
        }
        for child in &b.children {
            walk(child, document, out);
        }
    }
    let state = &engine.views[&view];
    let mut out = HashMap::new();
    walk(state.layout.as_ref().expect("layout"), state.document.as_ref().expect("document"), &mut out);
    out
}

/// The connected elements whose style differs between `before` and
/// `after`: present in both and different, or present in one only while
/// their parent is in both (the top of a subtree that gained or lost its
/// boxes).
fn changed(
    document: &Document,
    before: &HashMap<usize, ComputedStyle>,
    after: &HashMap<usize, ComputedStyle>,
) -> Vec<Rc<Node>> {
    let mut out = Vec::new();
    document.traverse(|n| {
        if !n.is_element() {
            return;
        }
        let raw = n.id.raw();
        let differs = match (before.get(&raw), after.get(&raw)) {
            (Some(a), Some(b)) => !same_computed_style(a, b),
            (None, None) => false,
            _ => n.parent().is_some_and(|p| before.contains_key(&p.id.raw()) && after.contains_key(&p.id.raw())),
        };
        if differs {
            out.push(n.clone());
        }
    });
    out
}

/// Covered by the set, or under a changed parent the set covers: an
/// inherited change found by the restyle carries on to the children
/// (note §3.2).
fn covered(set: &DirtySet, node: &Rc<Node>, changed: &[Rc<Node>]) -> bool {
    set.covers_style(node)
        || node.parent().is_some_and(|p| changed.iter().any(|c| Rc::ptr_eq(c, &p)) && covered(set, &p, changed))
}

const PAGE: &str = "<!DOCTYPE html><html><head><style>\
    body{margin:0;font:16px/20px sans-serif}\
    .on{color:red}\
    #picked{background:yellow}\
    [data-state=open]{height:40px}\
    .dark .item{color:white;background:black}\
    .sel + li{color:green} .sel ~ li{font-weight:bold}\
    li:nth-child(odd){background:#eee}\
    .box:empty{height:7px}\
    .card{padding:4px} .card > .title{font-size:20px}\
    </style></head><body>\
    <div id=wrap>\
      <div id=s1 class=card><div class=title>Title <b>bold</b></div><p id=p1>Para</p></div>\
      <div id=s2 class=item>Item</div>\
      <div id=theme><span class=item id=i1>a</span><span class=item>b</span></div>\
      <ul id=list><li id=l1>one</li><li id=l2>two</li><li id=l3>three</li><li id=l4>four</li></ul>\
      <div id=e1 class=box></div>\
      <div id=d1 data-state=closed>d</div>\
      <div id=other>o</div>\
    </div></body></html>";

fn load(page: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 600 }).expect("view");
    engine.load_html(view, page).expect("load_html");
    (engine, view)
}

/// Each element's raw NodeId, by its id attribute.
type ById = HashMap<String, usize>;

/// Run `script` against `page` and check the superset invariant. Returns
/// the set and the NodeId of each element by id, for case checks.
fn check(page: &str, script: &str) -> (DirtySet, ById) {
    check_after(page, "", script)
}

/// [`check`], after a `setup` script that writes nothing to the DOM.
fn check_after(page: &str, setup: &str, script: &str) -> (DirtySet, ById) {
    let (mut engine, view) = load(page);
    if !setup.is_empty() {
        engine.execute_script(view, setup).expect("setup");
    }
    let before = styles(&engine, view);
    engine.execute_script(view, script).expect("script");
    let set = engine.views[&view].last_dirty.clone().expect("the relayout logged a set");
    let after = styles(&engine, view);
    let document = engine.views[&view].document.clone().expect("document");
    let changed = changed(&document, &before, &after);
    let missed: Vec<String> = changed
        .iter()
        .filter(|n| !covered(&set, n, &changed))
        .map(|n| format!("{}#{}", n.tag_name().unwrap_or("?"), n.get_attribute("id").unwrap_or("")))
        .collect();
    assert!(missed.is_empty(), "`{script}` restyled {missed:?} outside {set:?}");
    let mut ids = ById::new();
    document.traverse(|n| {
        if let Some(id) = n.get_attribute("id") {
            ids.entry(id.to_string()).or_insert(n.id.raw());
        }
    });
    (set, ids)
}

fn style_bits(set: &DirtySet) -> usize {
    set.nodes.values().filter(|bits| *bits & (STYLE | SUBTREE_STYLE) != 0).count()
}

#[test]
fn style_attribute_writes_dirty_the_element() {
    let (set, ids) = check(PAGE, "document.getElementById('s1').style.color = 'blue';");
    assert_eq!(set.would_path(), "subtree");
    assert_eq!(set.bits(ids["s1"]), STYLE | PAINT);
    assert_eq!(set.nodes.len(), 1);
    check(PAGE, "document.getElementById('s1').style.height = '50px';");
    check(PAGE, "document.getElementById('s1').removeAttribute('style');");
}

#[test]
fn class_add_and_remove_restyle_the_subject() {
    let (set, ids) = check(PAGE, "document.getElementById('other').classList.add('on');");
    assert_eq!(set.would_path(), "subtree");
    assert_eq!(set.bits(ids["other"]) & STYLE, STYLE);
    check(PAGE, "var o = document.getElementById('other'); o.className = 'on'; o.className = '';");
    // A class no selector names restyles nothing.
    let (set, _) = check(PAGE, "document.getElementById('other').classList.add('unused');");
    assert_eq!(style_bits(&set), 0, "{set:?}");
    assert_eq!(set.would_path(), "subtree");
}

#[test]
fn id_changes_restyle_by_old_and_new_id() {
    let (set, ids) = check(PAGE, "document.getElementById('other').id = 'picked';");
    assert_eq!(set.bits(ids["picked"]) & STYLE, STYLE);
    // Losing the id restyles too.
    let (set, ids) =
        check(&PAGE.replace("<div id=other>", "<div id=picked>"), "document.getElementById('picked').id = 'gone';");
    assert_eq!(set.bits(ids["gone"]) & STYLE, STYLE);
}

#[test]
fn data_attributes_restyle_only_where_a_selector_reads_them() {
    let (set, ids) = check(PAGE, "document.getElementById('d1').setAttribute('data-state', 'open');");
    assert_eq!(set.bits(ids["d1"]) & STYLE, STYLE);
    let (set, _) = check(PAGE, "document.getElementById('d1').setAttribute('data-unread', 'x');");
    assert!(set.nodes.is_empty(), "no selector reads data-unread: {set:?}");
    assert_eq!(set.would_path(), "subtree");
}

#[test]
fn other_attributes_restyle_the_element() {
    let (set, ids) = check(PAGE, "document.getElementById('other').hidden = true;");
    assert_eq!(set.bits(ids["other"]) & (STYLE | BOX), STYLE | BOX);
}

#[test]
fn ancestor_compounds_restyle_the_subtree() {
    let (set, ids) = check(PAGE, "document.getElementById('theme').classList.add('dark');");
    assert_eq!(set.bits(ids["theme"]) & SUBTREE_STYLE, SUBTREE_STYLE);
    check(PAGE, "document.getElementById('s1').classList.remove('card');");
}

#[test]
fn a_complex_is_member_restyles_the_subtree_of_its_ancestor_token() {
    let page = PAGE.replace(".dark .item{", ".item:is(.dark *){");
    let (set, ids) = check(&page, "document.getElementById('theme').classList.add('dark');");
    assert_eq!(set.would_path(), "subtree", "{set:?}");
    assert_eq!(set.bits(ids["theme"]) & SUBTREE_STYLE, SUBTREE_STYLE);
    check_after(&page, "", "document.getElementById('theme').classList.add('dark'); document.getElementById('theme').classList.remove('dark');");
}

#[test]
fn sibling_combinators_restyle_later_siblings() {
    let (set, ids) = check(PAGE, "document.getElementById('l2').classList.add('sel');");
    assert_eq!(set.bits(ids["l3"]) & STYLE, STYLE);
    assert_eq!(set.bits(ids["l4"]) & STYLE, STYLE);
    assert_eq!(set.bits(ids["l1"]), 0, "an earlier sibling is untouched");
}

#[test]
fn inserts_and_removes_restyle_positional_siblings() {
    let (set, ids) = check(
        PAGE,
        "var l = document.getElementById('list'); var n = document.createElement('li'); \
         n.textContent = 'zero'; l.insertBefore(n, l.firstChild);",
    );
    assert_eq!(set.bits(ids["list"]) & (BOX | LAYOUT), BOX | LAYOUT);
    assert_eq!(set.bits(ids["l1"]) & STYLE, STYLE, ":nth-child moves");
    check(PAGE, "var l = document.getElementById('l1'); l.parentNode.removeChild(l);");
    check(PAGE, "document.getElementById('wrap').appendChild(document.getElementById('l2'));");
    check(
        PAGE,
        "var e = document.createElement('div'); e.className = 'on'; \
         document.getElementById('other').appendChild(e);",
    );
}

#[test]
fn text_edits_and_empty() {
    let script = "document.getElementById('p1').firstChild.data = 'Changed text';";
    let (set, ids) = check(&PAGE.replace(".box:empty{height:7px}", ""), script);
    assert_eq!(set.bits(ids["p1"]), LAYOUT, "a text edit restyles nothing");
    // With an `:empty` rule, the parent is restyled.
    let (set, ids) = check(PAGE, script);
    assert_eq!(set.bits(ids["p1"]), STYLE | LAYOUT);
    let (set, ids) = check(PAGE, "document.getElementById('e1').textContent = 'now full';");
    assert_eq!(set.bits(ids["e1"]) & STYLE, STYLE, ":empty flips");
    check(PAGE, "document.getElementById('e1').appendChild(document.createTextNode('x'));");
}

#[test]
fn inner_html_restyles_the_new_children() {
    let (set, ids) =
        check(PAGE, "document.getElementById('s1').innerHTML = '<div class=title>New</div><span class=on>x</span>';");
    assert_eq!(set.bits(ids["s1"]) & (SUBTREE_STYLE | BOX), SUBTREE_STYLE | BOX);
    check(PAGE, "document.getElementById('theme').innerText = 'plain';");
}

#[test]
fn writes_the_index_cannot_place_are_full() {
    // A `:has()` whose subject is an earlier sibling (LAT6 scopes the rest).
    let has = PAGE.replace(".on{color:red}", ".on{color:red} .title:has(+ .hot){color:red}");
    let (set, _) = check(&has, "document.getElementById('p1').classList.add('hot');");
    assert_eq!(set.full, Some("anywhere"));
    let (set, _) = check(PAGE, "document.body.classList.add('on');");
    assert_eq!(set.full, Some("root_or_body"));
    let (set, _) = check(
        PAGE,
        "var s = document.createElement('style'); s.textContent = '.item{color:blue}'; \
         document.head.appendChild(s);",
    );
    assert_eq!(set.full, Some("sheet"));
    let (set, _) = check(PAGE, "document.querySelector('style').firstChild.data += '#other{color:red}';");
    assert_eq!(set.full, Some("sheet"));
}

/// `:has()` matches, so the anchor's style really changes when a child is
/// added under it or removed from it: the set covers it both ways, scoped
/// to the anchor since LAT6 (it was full).
#[test]
fn a_child_list_write_under_a_has_anchor_covers_the_anchor() {
    let has = PAGE.replace(".on{color:red}", ".on{color:red} .card:has(.hot){color:red}");
    let add = "var e = document.createElement('b'); e.className = 'hot'; e.id = 'hot'; \
               document.getElementById('p1').appendChild(e);";
    let (set, ids) = check(&has, add);
    assert_eq!(set.full, None, "{set:?}");
    assert_eq!(set.bits(ids["s1"]) & STYLE, STYLE);
    let (set, _) = check_after(&has, add, "document.getElementById('hot').remove();");
    assert_eq!(set.full, None, "{set:?}");
    // Without a `:has()` rule the same write stays a subtree restyle.
    let (set, _) = check(PAGE, add);
    assert_eq!(set.full, None);
}

#[test]
fn many_writes_in_one_flush() {
    check(
        PAGE,
        "var ids = ['s1','s2','i1','l1','l3','d1','other','e1']; \
         for (var i = 0; i < ids.length; i++) document.getElementById(ids[i]).style.width = (50 + i) + 'px'; \
         document.getElementById('theme').classList.toggle('dark'); \
         document.getElementById('l3').classList.add('sel');",
    );
}

#[test]
fn checkedness_restyles_state_readers() {
    let page = "<!DOCTYPE html><html><head><style>input:checked + label{color:red}</style></head><body>\
        <div id=w><input id=c type=checkbox><label id=lab>L</label></div></body></html>";
    let (set, ids) = check(page, "document.getElementById('c').checked = true;");
    assert_eq!(set.bits(ids["lab"]) & STYLE, STYLE);
}

/// The synthetic page under `websuite/perf/` is about 5k elements, and one
/// tick of its timer variant is 4-8 inline style writes, each a single
/// dirty node.
#[test]
fn synthetic_timer_page_ticks_dirty_a_handful_of_nodes() {
    let page = include_str!("../../../websuite/perf/restyle-5k/index.html");
    let d = Document::parse_html(page).unwrap();
    let mut elements = 0;
    d.traverse(|n| elements += n.is_element() as usize);
    assert!((4500..6000).contains(&elements), "{elements} elements");
    let timer = include_str!("../../../websuite/perf/restyle-5k-timer/index.html");
    let setup = timer.rsplit("<script>").next().and_then(|s| s.split("</script>").next()).expect("the timer script");
    let (set, _) = check_after(timer, setup, "tick();");
    assert_eq!(set.would_path(), "subtree", "{set:?}");
    assert!((4..=8).contains(&set.nodes.len()), "a tick writes 4-8 rows: {set:?}");
    assert!(set.nodes.values().all(|bits| *bits == STYLE | PAINT), "{set:?}");
}

// ---------------------------------------------------------------------------
// LAT6 (Pete's flag-on log, 11-05): walmart inserts or removes one element
// every ~280 ms, and each of those writes was `would_path=full
// full_reason=has` because some `:has()` rule was in the sheets. A
// child-list write, or a write to a token a `:has()` argument names, can
// change only the `:has()` subjects above the written node: those
// ancestors the compound carrying the `:has()` can match.
// ---------------------------------------------------------------------------

fn with_rule(rule: &str) -> String {
    PAGE.replace(".on{color:red}", &format!(".on{{color:red}} {rule}"))
}

const ADD_HOT: &str = "var e = document.createElement('b'); e.className = 'hot'; e.id = 'hot'; \
                       document.getElementById('p1').appendChild(e);";

#[test]
fn a_child_list_write_under_a_has_subject_restyles_the_subject() {
    let has = with_rule(".card:has(.hot){color:red}");
    let (set, ids) = check(&has, ADD_HOT);
    assert_eq!(set.full, None, "{set:?}");
    assert_eq!(set.bits(ids["s1"]) & STYLE, STYLE, "the card is the subject above the write");
    assert_eq!(set.bits(ids["wrap"]), 0, "#wrap is no .card");
    // The card, the new element and the parent (the sheet reads `:empty`).
    assert!(style_bits(&set) <= 3, "{set:?}");
    let (set, ids) = check_after(&has, ADD_HOT, "document.getElementById('hot').remove();");
    assert_eq!(set.full, None, "{set:?}");
    assert_eq!(set.bits(ids["s1"]) & STYLE, STYLE);
}

#[test]
fn a_class_a_has_argument_names_restyles_the_subjects_above_it() {
    let has = with_rule(".card:has(.hot){color:red}");
    let (set, ids) = check(&has, "document.getElementById('p1').classList.add('hot');");
    assert_eq!(set.full, None, "{set:?}");
    assert_eq!(set.bits(ids["s1"]) & STYLE, STYLE);
    // Off a card, nothing a `:has()` reads changed.
    let (set, ids) = check(&has, "document.getElementById('other').classList.add('hot');");
    assert_eq!(set.full, None, "{set:?}");
    assert_eq!(set.bits(ids["s1"]), 0);
}

/// The subject sits left of a combinator: what is under it is restyled.
#[test]
fn a_has_subject_left_of_a_combinator_restyles_its_subtree() {
    let has = with_rule(".card:has(.hot) .title{color:red} .card:has(.hot) + .item{color:blue}");
    let (set, ids) = check(&has, ADD_HOT);
    assert_eq!(set.full, None, "{set:?}");
    assert_eq!(set.bits(ids["s1"]) & SUBTREE_STYLE, SUBTREE_STYLE, "the title under the card");
    assert_eq!(set.bits(ids["s2"]) & STYLE, STYLE, "the item after the card");
    check_after(&has, ADD_HOT, "document.getElementById('hot').remove();");
}

/// A relative selector that starts with `+` or `~` names a subject beside
/// the ancestors: still the full path.
#[test]
fn a_sibling_relative_has_stays_full() {
    let has = with_rule(".title:has(+ .hot){color:red}");
    let (set, _) = check(&has, "document.getElementById('p1').classList.add('hot');");
    assert!(set.full.is_some(), "{set:?}");
    let (set, _) = check(&has, ADD_HOT);
    assert!(set.full.is_some(), "{set:?}");
    // And a `:has()` rule like that makes the scoped ones full too.
    let both = with_rule(".title:has(+ .hot){color:red} .card:has(.hot){color:red}");
    let (set, _) = check(&both, "document.getElementById('other').classList.add('hot');");
    assert!(set.full.is_some(), "{set:?}");
}

/// Walmart's shape: a 280 ms timer inserts and removes one badge on one of
/// 200 cards whose sheet has a `:has()` rule. Each tick's relayout is a
/// subtree restyle of the card and the badge, and the rule follows it.
#[test]
fn a_timer_toggling_one_node_under_a_has_rule_dirties_its_card() {
    let mut html = String::from(
        "<!DOCTYPE html><html><head><style>body{margin:0;font:14px/18px sans-serif}\
         .card{padding:4px;border:1px solid #ccc;color:black} .card:has(.badge){color:rgb(200, 0, 0)}\
         </style></head><body><div id=grid>",
    );
    for i in 0..200 {
        html.push_str(&format!("<div class=card id=c{i}><span>card {i}</span></div>"));
    }
    html.push_str(
        "</div><script>setInterval(function () { var c = document.getElementById('c7'); \
         var b = c.querySelector('.badge'); if (b) { b.remove(); } else { b = document.createElement('i'); \
         b.className = 'badge'; b.textContent = 'new'; c.appendChild(b); } }, 280);</script></body></html>",
    );
    let (mut engine, view) = load(&html);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let colour = |engine: &mut Engine| {
        engine.execute_script(view, "String(getComputedStyle(document.getElementById('c7')).color)").expect("script")
    };
    let card = {
        let document = engine.views[&view].document.clone().expect("document");
        document.get_element_by_id("c7").expect("c7").id.raw()
    };
    for (tick, want) in [(1, "rgb(200, 0, 0)"), (2, "rgb(0, 0, 0)"), (3, "rgb(200, 0, 0)")] {
        let timers = |engine: &Engine| engine.relayout_stats(view).expect("view").by_first_cause.get("timer");
        let before = timers(&engine);
        rt.block_on(engine.pump_live(view, 300));
        assert_eq!(timers(&engine), before + 1, "tick {tick}: one timer relayout");
        let set = engine.views[&view].last_dirty.clone().expect("a set");
        assert_eq!(set.would_path(), "subtree", "tick {tick}: {set:?}");
        assert_eq!(set.bits(card) & STYLE, STYLE, "tick {tick}: the card is the subject");
        assert!(style_bits(&set) <= 2, "tick {tick}: the card and the badge, not the page: {set:?}");
        assert!(colour(&mut engine).contains(want), "tick {tick}: {}", colour(&mut engine));
    }
}
