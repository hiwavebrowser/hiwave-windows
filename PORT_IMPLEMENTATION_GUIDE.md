# CSS Engine Port - Implementation Guide

**Date**: January 16, 2026
**Status**: Ready to implement
**Discovery**: CSS infrastructure EXISTS in Windows, just not connected to engine

---

## Key Discovery

**Windows ALREADY HAS**:
- ✅ `rustkit-cssparser` crate with `parse_stylesheet()`
- ✅ `Stylesheet` struct (rustkit-css line 1023)
- ✅ `Rule` struct (rustkit-css line 1016)
- ✅ `Declaration` struct (rustkit-css line 1008)
- ✅ `Stylesheet::parse()` method (rustkit-css line 1034)

**What's MISSING**: Engine doesn't USE these!

---

## Functions to Port from macOS to Windows

### 1. extract_stylesheets() - SIMPLE
**macOS Location**: rustkit-engine/src/lib.rs line 2710
**Size**: ~30 lines
**Purpose**: Extract CSS from `<style>` tags

```rust
fn extract_stylesheets(&self, document: &Document) -> Vec<Stylesheet> {
    let mut stylesheets = Vec::new();
    let style_elements = document.get_elements_by_tag_name("style");
    for style_el in style_elements {
        let mut css_text = String::new();
        for child in style_el.children() {
            if let NodeType::Text(text) = &child.node_type {
                css_text.push_str(text);
            }
        }
        if !css_text.is_empty() {
            match Stylesheet::parse(&css_text) {
                Ok(stylesheet) => stylesheets.push(stylesheet),
                Err(e) => warn!(?e, "Failed to parse stylesheet"),
            }
        }
    }
    stylesheets
}
```

---

### 2. selector_matches() - COMPLEX
**macOS Location**: rustkit-engine/src/lib.rs line 2987
**Size**: ~586 lines (ends at line 3573)
**Purpose**: Check if CSS selector matches an element

**Signature**:
```rust
fn selector_matches(
    &self,
    selector: &str,
    tag_name: &str,
    attributes: &HashMap<String, String>,
    ancestors: &[(String, Vec<String>, Option<String>)],
    siblings: &[(String, Vec<String>, Option<String>)],
    element_index: usize,
    sibling_count: usize,
) -> bool
```

**Supports**:
- Element selectors: `div`, `span`, `p`
- Class selectors: `.container`
- ID selectors: `#main`
- Universal: `*`
- Descendant: `div span`
- Child: `div > span`
- Adjacent: `div + p`
- Sibling: `div ~ p`
- Attribute: `[type="text"]`
- Pseudo-classes: `:hover`, `:first-child`, `:nth-child(n)`

---

### 3. selector_specificity() - SIMPLE
**macOS Location**: rustkit-engine/src/lib.rs line 3573
**Size**: ~50 lines
**Purpose**: Calculate specificity for cascade resolution

**Returns**: `(id_count, class_count, tag_count)`

---

### 4. apply_style_property() - LARGE
**macOS Location**: rustkit-engine/src/lib.rs line 1776
**Size**: ~1000+ lines (handles 100+ CSS properties)
**Purpose**: Apply a single CSS property to ComputedStyle

**Must parse**: 70+ properties minimum

---

### 5. Modified compute_style_for_element()
**macOS Location**: rustkit-engine/src/lib.rs line 1404
**Windows Location**: rustkit-engine/src/lib.rs line 925

**Changes Needed**:
1. Add `stylesheets: &[Stylesheet]` parameter
2. Add `css_vars: &HashMap<String, String>` parameter
3. Add `ancestors: &[...]` parameter
4. After tag defaults, add rule matching (lines 1709-1748 from macOS)

---

## Implementation Order

### Step 1: Add Imports (5 min)
**Windows File**: `rustkit-engine/src/lib.rs` line ~24

**Change**:
```rust
// Before:
use rustkit_css::ComputedStyle;

// After:
use rustkit_css::{ComputedStyle, Stylesheet, Rule};
```

---

### Step 2: Port extract_stylesheets() (15 min)
**Windows File**: `rustkit-engine/src/lib.rs` after line 1069

**Action**: Copy function from macOS line 2710-2739

---

### Step 3: Port selector_specificity() (30 min)
**Windows File**: `rustkit-engine/src/lib.rs` after extract_stylesheets

**Action**: Copy function from macOS line 3573

---

### Step 4: Port selector_matches() (2-3 hours)
**Windows File**: `rustkit-engine/src/lib.rs` after selector_specificity

**Action**: Copy function from macOS line 2987-3573 (~586 lines)
**Note**: This is the largest single function

---

### Step 5: Port apply_style_property() (4-5 hours)
**Windows File**: `rustkit-engine/src/lib.rs` after selector_matches

**Action**:
1. Read macOS version from line 1776
2. Port property-by-property
3. Start with top 20 most common properties
4. Add more incrementally

**Top 20 Properties** (implement first):
1. display
2. position
3. width, height
4. margin-top/right/bottom/left
5. padding-top/right/bottom/left
6. font-size
7. font-family
8. font-weight
9. color
10. background-color
11. border-width/style/color
12. text-align
13. line-height
14. flex-direction
15. justify-content
16. align-items
17. grid-template-columns
18. grid-template-rows

---

### Step 6: Modify compute_style_for_element() (1 hour)
**Windows File**: `rustkit-engine/src/lib.rs` line 925

**Changes**:
1. Add parameters: `stylesheets: &[Stylesheet]`, `css_vars: &HashMap<String, String>`, `ancestors: &[...]`
2. After tag defaults (line ~1000), insert rule matching code from macOS lines 1700-1748
3. Keep inline style application at end

---

### Step 7: Modify build_layout_from_document() (30 min)
**Windows File**: `rustkit-engine/src/lib.rs` line 788

**Changes**:
1. Call `extract_stylesheets(document)` at start
2. Pass stylesheets to `build_layout_from_node`
3. Thread stylesheets through recursion

---

### Step 8: Test (2 hours)
Create test HTML files:
```html
<!DOCTYPE html>
<html>
<head>
  <style>
    body { background-color: #fafafa; }
    .test { color: blue; font-size: 20px; }
    #main { color: red; }
  </style>
</head>
<body>
  <div class="test">Should be blue 20px</div>
  <div id="main">Should be red</div>
</body>
</html>
```

Test with hiwave-smoke and verify styles apply.

---

## Code Locations Reference

### macOS (Source):
- **extract_stylesheets**: line 2710-2739
- **compute_style_for_element**: line 1404-1756
- **Rule matching logic**: line 1700-1748
- **selector_matches**: line 2987-3573
- **selector_specificity**: line 3573+
- **apply_style_property**: line 1776+
- **apply_inline_style**: line 1759-1773

### Windows (Target):
- **Imports**: line ~24 (add Stylesheet, Rule)
- **build_layout_from_document**: line 788 (call extract_stylesheets)
- **compute_style_for_element**: line 925 (add rule matching)
- **apply_inline_style**: line 1011-1069 (keep, enhance)
- **New functions**: Add after line 1069

---

## Testing Strategy

### Phase 1: Basic Selectors
```html
<style>
  div { color: red; }
  .class { color: blue; }
  #id { color: green; }
</style>
```
**Expected**: Colors should apply based on selectors

### Phase 2: Specificity
```html
<style>
  div { color: red; }
  .class { color: blue; }
  #id { color: green; }
</style>
<div id="id" class="class">Should be green (ID wins)</div>
```

### Phase 3: Box Model
```html
<style>
  .box {
    width: 200px;
    height: 100px;
    margin: 10px;
    padding: 20px;
    background-color: lightblue;
  }
</style>
```

### Phase 4: Flexbox
```html
<style>
  .container {
    display: flex;
    justify-content: center;
    align-items: center;
  }
</style>
```

### Phase 5: Grid
```html
<style>
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
    gap: 10px;
  }
</style>
```

---

## Estimated Timeline

- **Step 1-3** (Imports, extract, specificity): 1 hour
- **Step 4** (selector_matches): 3 hours
- **Step 5** (apply_style_property): 5 hours
- **Step 6-7** (Integration): 2 hours
- **Step 8** (Testing): 2 hours

**Total**: ~13 hours of focused implementation

**Phased Approach**: Can stop after Step 5 with top 20 properties, test, then add more properties incrementally.

---

## Resume Points

**If context runs out, resume from**:
1. Port selector_matches from macOS line 2987
2. Port selector_specificity from macOS line 3573
3. Port apply_style_property from macOS line 1776
4. Test with websuite test cases

**Critical Files**:
- macOS source: `P:\petes_code\ClaudeCode\hiwave\hiwave-macos\crates\rustkit-engine\src\lib.rs`
- Windows target: `P:\petes_code\ClaudeCode\hiwave\hiwave-windows\crates\rustkit-engine\src\lib.rs`
- Progress tracker: `CSS_ENGINE_PORT_PROGRESS.md`
- This guide: `PORT_IMPLEMENTATION_GUIDE.md`

---

**Last Updated**: January 16, 2026
**Next Action**: Start Step 1 - Add imports
