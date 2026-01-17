# CSS Engine Debugging - Evening Session
**Date**: January 16, 2026, Evening
**Status**: DEBUGGING IN PROGRESS - CSS not being applied

---

## Problem Summary

CSS engine integration completed and compiles successfully, BUT when testing with `hiwave-smoke`, the CSS is **NOT being applied**:

- All text renders at 16px (default) instead of specified sizes
- No colors from CSS are applied
- Background colors not working
- Flexbox/Grid layouts not activating

## What Was Done

### Integration Completed (Committed as 7cf57f3)
1. Added stylesheet extraction - `extract_stylesheets()`
2. Added selector matching - `selector_matches()` (558 lines)
3. Added specificity calculation - `selector_specificity()`
4. Added CSS property application - `apply_style_property()` (50+ properties)
5. Added grid helpers - `parse_grid_template()`, etc.
6. Integrated into `compute_style_for_element()` and `build_layout_from_document()`

### Test Attempt
Created `test_css_basic.html` with:
- h1 with color: red, font-size: 36px
- .blue-text with color: blue, font-size: 20px
- #green-box with green text, light green background
- Flexbox container
- Grid container

**Expected**: Styled page with colors, different font sizes, layouts
**Actual**: Plain page, all text 16px, default colors, no layouts

**Test Command**:
```bash
./target/release/hiwave-smoke.exe --html-file test_css_basic.html --dump-frame css_test_output.png --duration-ms 2000
```

## Root Cause Investigation

### Observation from Logs
From `bf980ec.output`:
```
Layout: text command ... text=CSS Engine Test ... font_size=16.0
Layout: text command ... text=This text should be BLUE and 20px ... font_size=16.0
```

All text is rendering at **16.0px** (the default), not the CSS-specified sizes.

### Debug Logging Added
Added `info!()` logs to track CSS processing:

1. **In `build_layout_from_document()` (line 791-794)**:
   ```rust
   info!(stylesheet_count = stylesheets.len(), "CSS: Extracted stylesheets");
   for (i, stylesheet) in stylesheets.iter().enumerate() {
       info!(index = i, rule_count = stylesheet.rules.len(), "CSS: Stylesheet rules");
   }
   ```

2. **In `extract_stylesheets()` (line 1154)**:
   ```rust
   info!(style_element_count = style_elements.len(), "CSS: Found <style> elements");
   ```

3. **In `compute_style_for_element()` (lines 1063-1067)**:
   ```rust
   if !matching_rules.is_empty() {
       info!(tag = tag_name, matched_rules = matching_rules.len(), "CSS: Rules matched for element");
   }
   for (rule, spec, _) in matching_rules {
       info!(selector = rule.selector.as_str(), specificity = ?spec, "CSS: Applying rule");
       // ...
   }
   ```

### Discovery: NO CSS LOGS APPEAR
When running test and filtering for "CSS:", **ZERO logs appear**.

This means:
- Either `extract_stylesheets()` is returning empty vector (0 stylesheets found)
- OR there's a problem earlier in the chain

## Hypothesis: Style Elements Not Being Found

**Most likely cause**: `document.get_elements_by_tag_name("style")` is returning empty vector.

**Possible reasons**:
1. `<style>` tags are being filtered out during DOM parsing (marked as "hidden")
2. `get_elements_by_tag_name()` implementation issue
3. DOM structure not what we expect

**Evidence from logs**:
```
DOM: grandchild of root index=0 tag=head
DOM: grandchild of root index=1 tag=body
```

The `<head>` element exists, and `<style>` should be a child of `<head>`.

### Code Review: Hidden Elements

From `build_layout_from_node()` (line 878-895):
```rust
// Skip rendering for certain elements
let is_hidden = matches!(
    tag_name.to_lowercase().as_str(),
    "head" | "title" | "meta" | "link" | "script" | "style" | "noscript"
);

if is_hidden {
    // Return an empty block for hidden elements
    return LayoutBox::new(BoxType::Block, ComputedStyle::new());
}
```

**CRITICAL BUG FOUND**: `<style>` elements are marked as `is_hidden` and return early with empty layout box!

This prevents them from being processed, BUT `extract_stylesheets()` is called **before** `build_layout_from_node()`, so this shouldn't affect stylesheet extraction.

## Next Steps to Debug

### Step 1: Verify Style Element Count
Run with new logging to see:
```bash
./target/release/hiwave-smoke.exe --html-file test_css_basic.html --dump-frame test.png --duration-ms 2000 2>&1 | grep "CSS:"
```

Expected output:
```
CSS: Found <style> elements style_element_count=1
CSS: Extracted stylesheets stylesheet_count=1
CSS: Stylesheet rules index=0 rule_count=15
CSS: Rules matched for element tag=h1 matched_rules=1
CSS: Applying rule selector="h1" specificity=(0, 0, 1)
...
```

### Step 2: If style_element_count=0
The problem is in `document.get_elements_by_tag_name("style")`.

**Action**:
1. Check `rustkit-dom` implementation of `get_elements_by_tag_name()`
2. Verify it does recursive search (not just direct children)
3. Test with direct DOM inspection logging

**Workaround**: Manually traverse DOM tree in `extract_stylesheets()`:
```rust
fn extract_stylesheets(&self, document: &Document) -> Vec<Stylesheet> {
    let mut stylesheets = Vec::new();

    // Get <html> element
    if let Some(html) = document.document_element() {
        // Get <head> element (first child of <html>)
        for child in html.children() {
            if let NodeType::Element { tag_name, .. } = &child.node_type {
                if tag_name.to_lowercase() == "head" {
                    // Search for <style> in <head>
                    self.extract_styles_from_node(&child, &mut stylesheets);
                }
            }
        }
    }

    stylesheets
}

fn extract_styles_from_node(&self, node: &Rc<Node>, stylesheets: &mut Vec<Stylesheet>) {
    if let NodeType::Element { tag_name, .. } = &node.node_type {
        if tag_name.to_lowercase() == "style" {
            // Extract CSS text from this node
            let mut css_text = String::new();
            for child in node.children() {
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
    }

    // Recurse into children
    for child in node.children() {
        self.extract_styles_from_node(&child, stylesheets);
    }
}
```

### Step 3: If style_element_count>0 but stylesheet_count=0
The problem is in CSS parsing (`Stylesheet::parse()`).

**Action**:
1. Log the CSS text being passed to parse: `info!(css_len = css_text.len(), css_preview = &css_text[..100.min(css_text.len())], "CSS: Parsing text");`
2. Check for parse errors in logs (should see `warn!` messages)
3. Test `Stylesheet::parse()` directly with simple CSS

### Step 4: If stylesheets extracted but matched_rules=0
The problem is in selector matching.

**Action**:
1. Log all selectors being tested: `info!(selector = rule.selector.as_str(), tag = tag_name, "CSS: Testing selector");`
2. Verify selector_matches() logic
3. Test simple selectors first (element, class, ID)

## Test File Contents

`test_css_basic.html`:
```html
<!DOCTYPE html>
<html>
<head>
    <style>
        body {
            background-color: #f0f0f0;
            margin: 20px;
        }

        h1 {
            color: #ff0000;
            font-size: 36px;
        }

        .blue-text {
            color: blue;
            font-size: 20px;
        }

        #green-box {
            color: green;
            background-color: #e0ffe0;
            padding: 15px;
            margin: 10px;
        }

        /* ... more rules ... */
    </style>
</head>
<body>
    <h1>CSS Engine Test</h1>
    <div>Default div text (should be black on light gray background)</div>
    <div class="blue-text">This text should be BLUE and 20px</div>
    <div id="green-box">This should have green text on light green background with padding</div>
    <!-- ... more content ... -->
</body>
</html>
```

## Current Code State

**Files Modified (uncommitted)**:
- `hiwave-windows/crates/rustkit-engine/src/lib.rs` - Added debug logging

**Last Commit**: `7cf57f3` - CSS engine integration complete

**Build Status**: ✅ Compiles cleanly

**Test Status**: ❌ CSS not being applied

## Resume Instructions

1. **Kill any running hiwave-smoke processes**
2. **Rebuild**: `cargo build --release -p hiwave-smoke`
3. **Run with logging**:
   ```bash
   cd hiwave-windows
   ./target/release/hiwave-smoke.exe --html-file test_css_basic.html --dump-frame test.png --duration-ms 2000 2>&1 | tee css_debug_full.log
   ```
4. **Check for CSS logs**:
   ```bash
   grep "CSS:" css_debug_full.log
   ```
5. **Follow debugging steps above** based on what logs appear

## Key Questions to Answer

1. **Are `<style>` elements being found?** Look for: `CSS: Found <style> elements style_element_count=X`
2. **Are stylesheets being parsed?** Look for: `CSS: Extracted stylesheets stylesheet_count=X`
3. **Are rules matching elements?** Look for: `CSS: Rules matched for element tag=X matched_rules=X`
4. **Are properties being applied?** Look for: `CSS: Applying rule selector=X`

## Likely Root Cause

**Most probable**: `document.get_elements_by_tag_name("style")` is not finding the style elements because:
- The method doesn't do recursive search
- OR it's case-sensitive and we need "STYLE"
- OR style elements are somehow not in the DOM tree

**Fix**: Implement manual DOM traversal in `extract_stylesheets()` to search for `<style>` tags in `<head>`.

---

**Last Updated**: January 16, 2026, 8:30 PM
**Status**: Awaiting debug log output
**Next Action**: Run test with debug logging and analyze results
