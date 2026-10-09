//! Intl in the page-script environment (web_intl.js): an en-US baseline for
//! NumberFormat, DateTimeFormat, PluralRules, Collator, ListFormat,
//! RelativeTimeFormat and the locale helpers, and the Number/Date
//! `toLocale*String` methods routed through them. Every expected string is
//! what Chrome prints for the same en-US call. Its own test module so it does
//! not collide with the other families' tests in `lib.rs`.

use super::*;

fn bindings() -> DomBindings {
    DomBindings::new(JsRuntime::new().unwrap()).unwrap()
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(b) => b.to_string(),
        JsValue::Number(n) => n.to_string(),
        JsValue::Null => "null".to_string(),
        JsValue::Undefined => "undefined".to_string(),
        other => panic!("{script} evaluated to {other:?}"),
    }
}

/// A fixed instant in UTC, 2024-01-15 13:05:09.123Z (a Monday).
const UTC_DATE: &str = "var d = new Date(Date.UTC(2024, 0, 15, 13, 5, 9, 123)); ";
/// The same wall-clock time in the default zone, so the expected strings
/// hold whatever zone the test runs in.
const LOCAL_DATE: &str = "var d = new Date(2024, 0, 15, 13, 5, 9, 123); ";

#[test]
fn intl_is_an_object_with_its_constructors() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "[typeof Intl, Object.prototype.toString.call(Intl), typeof Intl.NumberFormat, \
              typeof Intl.DateTimeFormat, typeof Intl.PluralRules, typeof Intl.Collator, \
              typeof Intl.ListFormat, typeof Intl.RelativeTimeFormat, typeof Intl.Segmenter, \
              typeof Intl.getCanonicalLocales].join(',')"
        ),
        "object,[object Intl],function,function,function,function,function,function,function,function"
    );
}

#[test]
fn number_format_decimal_groups_and_rounds() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "var nf = new Intl.NumberFormat('en-US'); \
             [nf.format(1234.5), nf.format(1234567.891), nf.format(0.1234), nf.format(-1234.5), \
              nf.format(0), nf.format(999), nf.format(1e21), nf.format(NaN), nf.format(Infinity), \
              nf.format(-Infinity), nf.format('1234.5'), nf.format(12345678901234567890n), \
              nf.format(1.0005)].join('|')"
        ),
        "1,234.5|1,234,567.891|0.123|-1,234.5|0|999|1,000,000,000,000,000,000,000|NaN|∞|-∞|1,234.5|12,345,678,901,234,567,890|1.001"
    );
    // `format` is a bound getter: pages pass it around unbound.
    assert_eq!(
        ev(
            &b,
            "var f = new Intl.NumberFormat().format; [1, 22, 1000].map(f).join(' ')"
        ),
        "1 22 1,000"
    );
    // Callable without `new`.
    assert_eq!(
        ev(&b, "Intl.NumberFormat('en-US').format(1e6)"),
        "1,000,000"
    );
}

#[test]
fn number_format_grouping_and_fraction_digits() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "[new Intl.NumberFormat('en-US', { useGrouping: false }).format(1234567.5), \
              new Intl.NumberFormat('en-US', { minimumFractionDigits: 2 }).format(5), \
              new Intl.NumberFormat('en-US', { minimumFractionDigits: 2 }).format(1.23456), \
              new Intl.NumberFormat('en-US', { maximumFractionDigits: 0 }).format(2.5), \
              new Intl.NumberFormat('en-US', { maximumFractionDigits: 1 }).format(1.25), \
              new Intl.NumberFormat('en-US', { minimumFractionDigits: 5 }).format(1), \
              new Intl.NumberFormat('en-US', { maximumFractionDigits: 2 }).format(1.005), \
              new Intl.NumberFormat('en-US', { minimumIntegerDigits: 3 }).format(7), \
              new Intl.NumberFormat('en-US', { maximumSignificantDigits: 3 }).format(123456), \
              new Intl.NumberFormat('en-US', { maximumFractionDigits: 2 }).format(-0.001)].join('|')"
        ),
        "1234567.5|5.00|1.235|3|1.3|1.00000|1.01|007|123,000|-0"
    );
    assert_eq!(
        ev(
            &b,
            "var t; try { new Intl.NumberFormat('en-US', { minimumFractionDigits: 3, maximumFractionDigits: 1 }); t = 'no throw'; } \
             catch (e) { t = e.name; } t"
        ),
        "RangeError"
    );
}

#[test]
fn number_format_percent_and_currency() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "function c(code, n) { return new Intl.NumberFormat('en-US', { style: 'currency', currency: code }).format(n); } \
             [new Intl.NumberFormat('en-US', { style: 'percent' }).format(0.256), \
              new Intl.NumberFormat('en-US', { style: 'percent', minimumFractionDigits: 1 }).format(0.256), \
              new Intl.NumberFormat('en-US', { style: 'percent' }).format(12.5), \
              c('USD', 1234.5), c('EUR', 1234.5), c('GBP', 1234.5), c('JPY', 1234.5), \
              c('USD', -5), c('usd', 0.5), c('USD', 1e6)].join('|')"
        ),
        "26%|25.6%|1,250%|$1,234.50|€1,234.50|£1,234.50|¥1,235|-$5.00|$0.50|$1,000,000.00"
    );
    assert_eq!(
        ev(
            &b,
            "var t; try { new Intl.NumberFormat('en-US', { style: 'currency' }); t = 'no throw'; } \
             catch (e) { t = e.name; } t"
        ),
        "TypeError"
    );
}

#[test]
fn number_format_compact_notation() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "var s = new Intl.NumberFormat('en-US', { notation: 'compact' }); \
             var l = new Intl.NumberFormat('en-US', { notation: 'compact', compactDisplay: 'long' }); \
             [s.format(999), s.format(1000), s.format(1234), s.format(12345), s.format(123456), \
              s.format(1500000), s.format(2.5e9), s.format(1.2e12), s.format(-1234), s.format(1.5), \
              l.format(1234), l.format(1500000), l.format(2e9)].join('|')"
        ),
        "999|1K|1.2K|12K|123K|1.5M|2.5B|1.2T|-1.2K|1.5|1.2 thousand|1.5 million|2 billion"
    );
}

#[test]
fn number_format_to_parts() {
    let b = bindings();
    let parts = |script: &str| {
        format!("{script}.map(function (p) {{ return p.type + '=' + p.value; }}).join(' ')")
    };
    assert_eq!(
        ev(
            &b,
            &parts("new Intl.NumberFormat('en-US').formatToParts(-1234.5)")
        ),
        "minusSign=- integer=1 group=, integer=234 decimal=. fraction=5"
    );
    assert_eq!(
        ev(
            &b,
            &parts("new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).formatToParts(5)")
        ),
        "currency=$ integer=5 decimal=. fraction=00"
    );
    assert_eq!(
        ev(
            &b,
            &parts("new Intl.NumberFormat('en-US', { style: 'percent' }).formatToParts(0.5)")
        ),
        "integer=50 percentSign=%"
    );
    assert_eq!(
        ev(
            &b,
            &parts("new Intl.NumberFormat('en-US', { notation: 'compact' }).formatToParts(1500)")
        ),
        "integer=1 decimal=. fraction=5 compact=K"
    );
}

#[test]
fn number_format_resolved_options() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "var o = new Intl.NumberFormat('en-US').resolvedOptions(); \
             [o.locale, o.numberingSystem, o.style, o.minimumIntegerDigits, o.minimumFractionDigits, \
              o.maximumFractionDigits, o.notation, o.signDisplay].join(',')"
        ),
        "en-US,latn,decimal,1,0,3,standard,auto"
    );
    assert_eq!(
        ev(
            &b,
            "var o = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'jpy' }).resolvedOptions(); \
             var p = new Intl.NumberFormat(undefined, { style: 'percent' }).resolvedOptions(); \
             [o.style, o.currency, o.currencyDisplay, o.minimumFractionDigits, o.maximumFractionDigits, \
              p.locale, p.maximumFractionDigits].join(',')"
        ),
        "currency,JPY,symbol,0,0,en-US,0"
    );
}

#[test]
fn number_to_locale_string_goes_through_number_format() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "[(1234.5).toLocaleString(), (1234.5).toLocaleString('en-US'), \
              (1234.5).toLocaleString('en-US', { style: 'currency', currency: 'USD' }), \
              (0.75).toLocaleString('en-US', { style: 'percent' }), \
              (1234567).toLocaleString(undefined, { maximumFractionDigits: 0 }), \
              new Number(42000).toLocaleString()].join('|')"
        ),
        "1,234.5|1,234.5|$1,234.50|75%|1,234,567|42,000"
    );
}

#[test]
fn date_time_format_defaults_and_styles_in_utc() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            &format!(
                "{UTC_DATE} function f(o) {{ o.timeZone = 'UTC'; return new Intl.DateTimeFormat('en-US', o).format(d); }} \
                 [f({{}}), f({{ dateStyle: 'full' }}), f({{ dateStyle: 'long' }}), f({{ dateStyle: 'medium' }}), \
                  f({{ dateStyle: 'short' }}), f({{ timeStyle: 'long' }}), f({{ timeStyle: 'medium' }}), \
                  f({{ timeStyle: 'short' }}), f({{ dateStyle: 'medium', timeStyle: 'short' }}), \
                  f({{ dateStyle: 'short', timeStyle: 'short' }})].join('|')"
            )
        ),
        "1/15/2024|Monday, January 15, 2024|January 15, 2024|Jan 15, 2024|1/15/24|1:05:09 PM UTC|1:05:09 PM|1:05 PM|Jan 15, 2024, 1:05 PM|1/15/24, 1:05 PM"
    );
}

#[test]
fn date_time_format_component_options_in_utc() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            &format!(
                "{UTC_DATE} function f(o) {{ o.timeZone = 'UTC'; return new Intl.DateTimeFormat('en-US', o).format(d); }} \
                 [f({{ year: 'numeric', month: 'long', day: 'numeric' }}), \
                  f({{ month: 'short', day: 'numeric' }}), \
                  f({{ weekday: 'long' }}), f({{ weekday: 'short' }}), f({{ month: 'long' }}), \
                  f({{ year: 'numeric', month: 'long' }}), f({{ year: 'numeric', month: 'short' }}), \
                  f({{ month: '2-digit', day: '2-digit', year: 'numeric' }}), \
                  f({{ year: '2-digit', month: '2-digit', day: '2-digit' }}), \
                  f({{ month: 'numeric', day: 'numeric' }}), \
                  f({{ weekday: 'long', year: 'numeric', month: 'long', day: 'numeric' }}), \
                  f({{ weekday: 'short', month: 'short', day: 'numeric' }}), \
                  f({{ year: 'numeric' }}), \
                  f({{ hour: 'numeric', minute: '2-digit' }}), f({{ hour: '2-digit', minute: '2-digit' }}), \
                  f({{ hour: 'numeric' }}), f({{ hour: 'numeric', minute: '2-digit', hour12: false }}), \
                  f({{ hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }})].join('|')"
            )
        ),
        "January 15, 2024|Jan 15|Monday|Mon|January|January 2024|Jan 2024|01/15/2024|01/15/24|1/15|Monday, January 15, 2024|Mon, Jan 15|2024|1:05 PM|01:05 PM|1 PM|13:05|13:05:09"
    );
    // Midnight and noon on the 12-hour clock.
    assert_eq!(
        ev(
            &b,
            "var o = { timeZone: 'UTC', hour: 'numeric', minute: '2-digit' }; \
             [new Intl.DateTimeFormat('en-US', o).format(Date.UTC(2024, 0, 1, 0, 30)), \
              new Intl.DateTimeFormat('en-US', o).format(Date.UTC(2024, 0, 1, 12, 0))].join('|')"
        ),
        "12:30 AM|12:00 PM"
    );
}

#[test]
fn date_time_format_to_parts_and_resolved_options() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            &format!(
                "{UTC_DATE} new Intl.DateTimeFormat('en-US', {{ timeZone: 'UTC', year: 'numeric', month: 'numeric', day: 'numeric', \
                 hour: 'numeric', minute: '2-digit', second: '2-digit' }}).formatToParts(d)\
                 .map(function (p) {{ return p.type + '=' + p.value; }}).join('|')"
            )
        ),
        "month=1|literal=/|day=15|literal=/|year=2024|literal=, |hour=1|literal=:|minute=05|literal=:|second=09|literal= |dayPeriod=PM"
    );
    assert_eq!(
        ev(
            &b,
            "var o = new Intl.DateTimeFormat('en-US', { timeZone: 'utc' }).resolvedOptions(); \
             var s = new Intl.DateTimeFormat('en-US', { dateStyle: 'medium' }).resolvedOptions(); \
             [o.locale, o.calendar, o.numberingSystem, o.timeZone, o.year, o.month, o.day, String(o.hour), \
              s.dateStyle, String(s.year)].join(',')"
        ),
        "en-US,gregory,latn,UTC,numeric,numeric,numeric,undefined,medium,undefined"
    );
    // The default zone resolves to a zone name and formats a string.
    assert_eq!(
        ev(
            &b,
            "typeof new Intl.DateTimeFormat().resolvedOptions().timeZone + ',' + \
             typeof Intl.DateTimeFormat('en-US').format(0)"
        ),
        "string,string"
    );
    assert_eq!(
        ev(
            &b,
            "var t; try { new Intl.DateTimeFormat('en-US').format(new Date(NaN)); t = 'no throw'; } \
             catch (e) { t = e.name; } t"
        ),
        "RangeError"
    );
}

#[test]
fn date_to_locale_strings_go_through_date_time_format() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            &format!(
                "{LOCAL_DATE} [d.toLocaleDateString(), d.toLocaleTimeString(), d.toLocaleString(), \
                  d.toLocaleDateString('en-US'), d.toLocaleDateString('en-US', {{ month: 'long', day: 'numeric', year: 'numeric' }}), \
                  d.toLocaleTimeString('en-US', {{ hour: '2-digit', minute: '2-digit' }}), \
                  d.toLocaleDateString('en-US', {{ weekday: 'long' }}), \
                  new Date(NaN).toLocaleDateString()].join('|')"
            )
        ),
        "1/15/2024|1:05:09 PM|1/15/2024, 1:05:09 PM|1/15/2024|January 15, 2024|01:05 PM|Monday|Invalid Date"
    );
    assert_eq!(
        ev(
            &b,
            &format!("{UTC_DATE} d.toLocaleString('en-US', {{ timeZone: 'UTC' }})")
        ),
        "1/15/2024, 1:05:09 PM"
    );
}

#[test]
fn plural_rules_cardinal_and_ordinal() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "var p = new Intl.PluralRules('en-US'); [0, 1, 2, 1.5, -1, 100].map(function (n) { return p.select(n); }).join(',')"
        ),
        "other,one,other,other,one,other"
    );
    assert_eq!(
        ev(
            &b,
            "var o = new Intl.PluralRules('en-US', { type: 'ordinal' }); \
             [1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 101, 111, 112, 0].map(function (n) { return o.select(n); }).join(',')"
        ),
        "one,two,few,other,other,other,other,one,two,few,one,other,other,other"
    );
    assert_eq!(
        ev(
            &b,
            "var r = new Intl.PluralRules('en-US').resolvedOptions(); \
             [r.locale, r.type, r.pluralCategories.join('/')].join(',')"
        ),
        "en-US,cardinal,one/other"
    );
}

#[test]
fn collator_compares_with_sensitivity_and_numeric() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "var c = new Intl.Collator('en').compare; \
             [c('a', 'b'), c('b', 'a'), c('a', 'a'), c('a', 'B'), c('a', 'A'), c('a', '\\u00e1'), \
              c('item2', 'item10')].join(',')"
        ),
        "-1,1,0,-1,-1,-1,1"
    );
    assert_eq!(
        ev(
            &b,
            "function c(o, x, y) { return new Intl.Collator('en', o).compare(x, y); } \
             [c({ sensitivity: 'base' }, 'a', 'A'), c({ sensitivity: 'base' }, 'a', '\\u00e1'), \
              c({ sensitivity: 'accent' }, 'a', 'A'), c({ sensitivity: 'accent' }, 'a', '\\u00e1') !== 0, \
              c({ numeric: true }, 'item2', 'item10'), c({ numeric: true }, 'item10', 'item10')].join(',')"
        ),
        "0,0,0,true,-1,0"
    );
    assert_eq!(
        ev(
            &b,
            "['b', 'a', 'C', 'c'].sort(new Intl.Collator('en').compare).join('')"
        ),
        "abcC"
    );
    assert_eq!(
        ev(
            &b,
            "var o = new Intl.Collator('en-US', { numeric: true }).resolvedOptions(); \
             [o.locale, o.usage, o.sensitivity, o.numeric].join(',')"
        ),
        "en-US,sort,variant,true"
    );
}

#[test]
fn list_format_conjunction_disjunction_unit() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "var c = new Intl.ListFormat('en', { type: 'conjunction' }); \
             var d = new Intl.ListFormat('en', { type: 'disjunction' }); \
             var u = new Intl.ListFormat('en', { type: 'unit' }); \
             [c.format(['a', 'b', 'c']), c.format(['a', 'b']), c.format(['a']), c.format([]), \
              d.format(['a', 'b', 'c']), d.format(['a', 'b']), u.format(['a', 'b', 'c']), \
              new Intl.ListFormat('en-US').format(['x', 'y', 'z', 'w'])].join('|')"
        ),
        "a, b, and c|a and b|a||a, b, or c|a or b|a, b, c|x, y, z, and w"
    );
    assert_eq!(
        ev(
            &b,
            "new Intl.ListFormat('en').formatToParts(['a', 'b', 'c'])\
             .map(function (p) { return p.type + '=' + p.value; }).join('|')"
        ),
        "element=a|literal=, |element=b|literal=, and |element=c"
    );
}

#[test]
fn relative_time_format_numeric_and_auto() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "var r = new Intl.RelativeTimeFormat('en'); \
             [r.format(-1, 'day'), r.format(2, 'day'), r.format(1, 'hour'), r.format(-3, 'months'), \
              r.format(5, 'minutes'), r.format(-10, 'second'), r.format(1, 'week'), r.format(2, 'quarter'), \
              r.format(-1, 'year'), r.format(1000, 'day'), r.format(1.5, 'day')].join('|')"
        ),
        "1 day ago|in 2 days|in 1 hour|3 months ago|in 5 minutes|10 seconds ago|in 1 week|in 2 quarters|1 year ago|in 1,000 days|in 1.5 days"
    );
    assert_eq!(
        ev(
            &b,
            "var a = new Intl.RelativeTimeFormat('en', { numeric: 'auto' }); \
             [a.format(-1, 'day'), a.format(1, 'day'), a.format(0, 'day'), a.format(0, 'second'), \
              a.format(-1, 'year'), a.format(1, 'week'), a.format(0, 'year'), a.format(-2, 'day')].join('|')"
        ),
        "yesterday|tomorrow|today|now|last year|next week|this year|2 days ago"
    );
    assert_eq!(
        ev(
            &b,
            "var t; try { new Intl.RelativeTimeFormat('en').format(1, 'fortnight'); t = 'no throw'; } \
             catch (e) { t = e.name; } t"
        ),
        "RangeError"
    );
    assert_eq!(
        ev(
            &b,
            "var o = new Intl.RelativeTimeFormat('en-US').resolvedOptions(); [o.locale, o.style, o.numeric].join(',')"
        ),
        "en-US,long,always"
    );
}

#[test]
fn canonical_locales_and_supported_locales_of() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "[Intl.getCanonicalLocales('EN-us').join(','), Intl.getCanonicalLocales(['en-us', 'EN-US']).join(','), \
              Intl.getCanonicalLocales('zh-hant-tw').join(','), Intl.getCanonicalLocales().length, \
              Intl.NumberFormat.supportedLocalesOf('en-US').join(','), \
              Intl.DateTimeFormat.supportedLocalesOf([]).length].join('|')"
        ),
        "en-US|en-US|zh-Hant-TW|0|en-US|0"
    );
    assert_eq!(
        ev(
            &b,
            "var t; try { Intl.getCanonicalLocales('en_US'); t = 'no throw'; } catch (e) { t = e.name; } t"
        ),
        "RangeError"
    );
}

#[test]
fn intl_segmenter_grapheme_word_and_sentence() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "[typeof Intl.Segmenter, typeof Intl.Segmenter.supportedLocalesOf].join(',')"
        ),
        "function,function"
    );
    // resolvedOptions default
    assert_eq!(
        ev(
            &b,
            "var s = new Intl.Segmenter('en-US'); \
             var o = s.resolvedOptions(); \
             [o.locale, o.granularity].join(',')"
        ),
        "en-US,grapheme"
    );
    // options granularity
    assert_eq!(
        ev(
            &b,
            "var s = new Intl.Segmenter('en-US', { granularity: 'word' }); \
             s.resolvedOptions().granularity"
        ),
        "word"
    );
    // invalid granularity throws RangeError
    assert_eq!(
        ev(
            &b,
            "var t; try { new Intl.Segmenter('en', { granularity: 'invalid' }); t = 'no throw'; } \
             catch (e) { t = e.name; } t"
        ),
        "RangeError"
    );
    // segment iterating graphemes
    assert_eq!(
        ev(
            &b,
            "var seg = new Intl.Segmenter('en', { granularity: 'grapheme' }); \
             var res = []; \
             for (var item of seg.segment('Hello!')) { \
                 res.push(item.segment + '@' + item.index + '@' + (typeof item.isWordLike)); \
             } \
             res.join('|')"
        ),
        "H@0@undefined|e@1@undefined|l@2@undefined|l@3@undefined|o@4@undefined|!@5@undefined"
    );
    // segment iterating words with isWordLike
    assert_eq!(
        ev(
            &b,
            "var seg = new Intl.Segmenter('en', { granularity: 'word' }); \
             var res = []; \
             for (var item of seg.segment('Hello, world!')) { \
                 res.push(item.segment + ':' + item.isWordLike + '@' + item.index); \
             } \
             res.join('|')"
        ),
        "Hello:true@0|,:false@5| :false@6|world:true@7|!:false@12"
    );
    // containing(index)
    assert_eq!(
        ev(
            &b,
            "var seg = new Intl.Segmenter('en', { granularity: 'word' }); \
             var segments = seg.segment('Hello world'); \
             var c = segments.containing(7); \
             [c.segment, c.index, c.isWordLike].join(',')"
        ),
        "world,6,true"
    );
    // containing out of bounds returns undefined
    assert_eq!(
        ev(
            &b,
            "var seg = new Intl.Segmenter('en'); \
             var segments = seg.segment('abc'); \
             [String(segments.containing(-1)), String(segments.containing(3))].join(',')"
        ),
        "undefined,undefined"
    );
}
