//! `offsetParent` through tables (Z row C10 (c)).
//! A static element stops at the nearest `table`, `td` or `th`, by element
//! name and not by display; a positioned one walks past them. Offsets are from
//! the padding edge of the parent found.
//!
//! The rows are what Chromium 143.0.7499.4 (the parity oracle) reports for each hand-made
//! body: `<offsetParent id, or tag, or null> <offsetLeft> <offsetTop>` of #t,
//! written by `trench/z/tools/c10/offset_parent_gen.py` on the Atlas hub. Rows
//! that rest on font metrics are left out.

use super::*;

const CASES: &[(&str, &str, &str)] = &[
    ("in a td", r##"<table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "c 3 18"),
    ("in a th", r##"<table id=tb style="border-spacing:0"><tr><th id=c style="padding:3px;width:40px;height:40px"><div id=t style="width:10px;height:10px"></div></th></tr></table>"##, "c 3 18"),
    ("deep in a td", r##"<table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><div id=w style="padding:2px"><div id=w2 style="padding:1px"><div id=t style="width:10px;height:10px"></div></div></div></td></tr></table>"##, "c 6 18"),
    ("td with a border", r##"<table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;border:4px solid red"><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "c 3 18"),
    ("table with margin and border", r##"<table id=tb style="border-spacing:0;margin:7px;border:2px solid red"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "c 3 18"),
    ("table with spacing", r##"<table id=tb style="border-spacing:0;border-spacing:5px"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "c 3 18"),
    ("td is the target", r##"<table id=tb style="border-spacing:0;margin:5px"><tr><td id=t style="width:40px;height:40px"></td></tr></table>"##, "tb 0 0"),
    ("second td is the target", r##"<table id=tb style="border-spacing:0;margin:5px"><tr><td style="width:40px;height:40px"></td><td id=t style="width:40px;height:40px"></td></tr></table>"##, "tb 42 0"),
    ("th is the target", r##"<table id=tb style="border-spacing:0"><tr><th id=t style="width:40px;height:40px"></th></tr></table>"##, "tb 0 0"),
    ("tr is the target", r##"<table id=tb style="border-spacing:0;margin:5px"><tr id=t><td style="width:40px;height:40px"></td></tr></table>"##, "tb 0 0"),
    ("tbody is the target", r##"<table id=tb style="border-spacing:0;margin:5px"><tbody id=t><tr><td style="width:40px;height:40px"></td></tr></tbody></table>"##, "tb 0 0"),
    ("table is the target", r##"<table id=t style="margin:5px"><tr><td>x</td></tr></table>"##, "body 5 5"),
    ("relative target in a td", r##"<table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><div id=t style="width:10px;height:10px;position:relative"></div></td></tr></table>"##, "body 3 18"),
    ("relative target with offset in a td", r##"<table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><div id=t style="width:10px;height:10px;position:relative;left:5px;top:6px"></div></td></tr></table>"##, "body 8 24"),
    ("sticky target in a td", r##"<table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><div id=t style="width:10px;height:10px;position:sticky;top:0"></div></td></tr></table>"##, "body 3 18"),
    ("static target in a relative td", r##"<table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;position:relative"><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "c 3 18"),
    ("relative wrapper inside the td", r##"<table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><div id=w style="position:relative;padding:2px"><div id=t style="width:10px;height:10px"></div></div></td></tr></table>"##, "w 2 2"),
    ("td inside a relative wrapper", r##"<div id=w style="position:relative;padding:9px"><table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><div id=t style="width:10px;height:10px"></div></td></tr></table></div>"##, "c 3 18"),
    ("nested table", r##"<table id=tb style="border-spacing:0;"><tr id=tr><td id=c style="padding:3px;width:40px;height:40px;"><table id=tb2 style="border-spacing:0"><tr><td id=c2 style="padding:2px"><div id=t style="width:10px;height:10px"></div></td></tr></table></td></tr></table>"##, "c2 2 2"),
    ("td with display block", r##"<table id=tb style="border-spacing:0"><tr><td id=c style="display:block;padding:3px"><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "c 3 3"),
    ("td with display contents", r##"<table id=tb style="border-spacing:0"><tr><td id=c style="display:contents"><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "tb 0 0"),
    ("table with display block", r##"<table id=tb style="display:block;margin:5px"><tr><td id=c><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "c 1 1"),
    ("td made by script outside a table", r##"<div id=h></div><script>var d=document.createElement('td');d.id='c';d.style.padding='3px';d.innerHTML='<div id=t style="width:10px;height:10px"></div>';document.getElementById('h').appendChild(d)</script>"##, "c 3 3"),
    ("display table on a div", r##"<div id=w style="display:table;margin:5px"><div id=w2 style="display:table-cell;padding:3px"><div id=t style="width:10px;height:10px"></div></div></div>"##, "body 8 8"),
    ("tr with position", r##"<table id=tb style="border-spacing:0"><tr id=tr style="position:relative"><td id=c style="padding:3px"><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "c 3 3"),
    ("in a tr, no td", r##"<table id=tb style="border-spacing:0"><tr id=tr><div id=t style="width:10px;height:10px"></div></tr></table>"##, "body 0 0"),
    ("td display none", r##"<table id=tb style="border-spacing:0"><tr><td id=c style="display:none"><div id=t style="width:10px;height:10px"></div></td></tr></table>"##, "null 0 0"),
    ("control, no table", r##"<div id=w style="margin:6px;padding:4px"><div id=t style="width:10px;height:10px"></div></div>"##, "body 10 10"),
    ("control, relative", r##"<div id=w style="position:relative;margin:6px;padding:4px"><div id=t style="width:10px;height:10px"></div></div>"##, "w 4 4"),
];

const READ: &str = "(function () { var e = document.getElementById('t'); var o = e.offsetParent; \
    return [o ? (o.id || o.tagName.toLowerCase()) : 'null', e.offsetLeft, e.offsetTop].join(' '); })()";

fn wrong_rows(cases: &[(&str, &str, &str)]) -> Vec<String> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 800, 600))
        .expect("headless view");
    let mut wrong = Vec::new();
    for (name, body, want) in cases {
        let html = format!("<!doctype html><style>body{{margin:0}}</style><body>{body}");
        engine.load_html(view, &html).expect("load_html");
        let value = engine.execute_script(view, READ).expect("script");
        let got = value
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&value);
        if got != *want {
            wrong.push(format!("{name}: {got}, Chromium {want}"));
        }
    }
    wrong
}

#[test]
fn offset_parent_through_tables_matches_chromium() {
    let wrong = wrong_rows(CASES);
    assert!(
        wrong.is_empty(),
        "{} of {} wrong:\n{}",
        wrong.len(),
        CASES.len(),
        wrong.join("\n")
    );
}

/// A table with a top `<caption>`: Chromium's table box includes the caption,
/// so a row or cell is 12px down from the table's padding edge. HiWave's table
/// box starts below the caption, so it reads `tb 0 0`, and the table itself
/// reads 12px low. Not an offsetParent bug: the table wrapper box should hold
/// the caption (C10/C12 follow-up, PR #864 R1 finding 1).
const CAPTION_CASES: &[(&str, &str, &str)] = &[
    ("td in a captioned table", r##"<table id=tb style="border-spacing:0;margin:5px"><caption style="height:12px;padding:0"></caption><tr><td id=t style="width:40px;height:40px"></td></tr></table>"##, "tb 0 12"),
    ("tr in a captioned table", r##"<table id=tb style="border-spacing:0;margin:5px"><caption style="height:12px;padding:0"></caption><tr id=t><td style="width:40px;height:40px"></td></tr></table>"##, "tb 0 12"),
    ("captioned table is the target", r##"<table id=t style="border-spacing:0;margin:5px"><caption style="height:12px;padding:0"></caption><tr><td style="width:40px;height:40px"></td></tr></table>"##, "body 5 5"),
];

#[test]
#[ignore = "table box excludes a top caption (C10/C12 follow-up)"]
fn offset_parent_in_a_captioned_table_matches_chromium() {
    let wrong = wrong_rows(CAPTION_CASES);
    assert!(
        wrong.is_empty(),
        "{} of {} wrong:\n{}",
        wrong.len(),
        CAPTION_CASES.len(),
        wrong.join("\n")
    );
}
