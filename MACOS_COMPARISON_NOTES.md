# macOS vs Windows Implementation Comparison

**Generated**: January 16, 2026
**Purpose**: Track differences and gaps between macOS and Windows implementations

---

## CRITICAL DISCOVERY: CSS Infrastructure Already Exists in Windows!

**The Good News**: Windows ALREADY has most of the CSS parsing infrastructure!

### What Windows HAS:

1. **rustkit-cssparser crate** with `parse_stylesheet()` function ✅
2. **Stylesheet struct** in rustkit-css (line 1023) ✅
3. **Rule struct** in rustkit-css (line 1016) ✅
4. **Declaration struct** in rustkit-css (line 1008) ✅
5. **PropertyValue enum** in rustkit-css (line 997) ✅
6. **Stylesheet::parse()** method (line 1034) ✅
7. **Test for parse_stylesheet** (line 1301) ✅

### The Problem:

**The ENGINE doesn't USE these types!**

**Windows engine imports:**
```rust
use rustkit_css::ComputedStyle;  // ONLY THIS
```

**macOS engine imports:**
```rust
use rustkit_css::{ComputedStyle, Stylesheet, Rule, parse_display};
```

**Conclusion**: We don't need to port 4,000 lines of CSS parsing. We just need to:
1. Add imports for Stylesheet, Rule, etc.
2. Call extract_stylesheets() during DOM processing
3. Match rules against elements
4. Apply matched declarations to styles

This dramatically reduces the work!

---

## Revised Estimate

**Original Estimate**: 4,000 lines to port
**Actual Needed**: ~800 lines of integration code

**Why?**:
- CSS parsing already exists in rustkit-cssparser ✅
- Stylesheet/Rule types already exist ✅
- We just need to USE them in the engine

---

## What Needs to be Ported from macOS to Windows

### 1. Import Statements (5 lines)

**macOS** (line ~9):
```rust
use rustkit_css::{ComputedStyle, Stylesheet, Rule, parse_display};
```

**Windows** (line ~24):
```rust
use rustkit_css::ComputedStyle;
```

**Action**: Add Stylesheet and Rule to Windows imports.

---

### 2. ViewState Changes (10 lines)

**macOS** has `external_stylesheets: Vec<Stylesheet>` in ViewState
**Windows** does NOT

**Files**:
- macOS: rustkit-engine/src/lib.rs line ~162
- Windows: Need to add to ViewState struct

---

### 3. extract_stylesheets() Function (~50 lines)

**macOS** has this function to extract CSS from `<style>` tags.
**Windows** does NOT.

**macOS Source**: Need to find exact location
**Implementation**: Parse `<style>` element text content using Stylesheet::parse()

---

### 4. build_layout_from_document() Signature Change (5 lines)

**macOS**:
```rust
fn build_layout_from_document(&self, document: &Document, external_stylesheets: &[Stylesheet]) -> LayoutBox
```

**Windows**:
```rust
fn build_layout_from_document(&self, document: &Document) -> LayoutBox
```

**Action**: Add stylesheets parameter to Windows version.

---

### 5. compute_style_for_element() Enhancement (~400 lines)

This is the BIG change.

**Current Windows Flow**:
1. Create default ComputedStyle
2. Apply tag-specific styles (h1, p, div, etc.)
3. Apply inline style="" attribute
4. Return

**Needed Flow (like macOS)**:
1. Create default ComputedStyle
2. Apply tag-specific styles (user-agent stylesheet)
3. **Apply stylesheet rules that match this element** ← MISSING
4. Apply inline style="" attribute (highest specificity)
5. **Inherit from parent** ← PARTIALLY MISSING
6. Return

**Key Functions to Port**:
- `matches_selector()` - Check if selector matches element (~150 lines)
- `apply_rule()` - Apply CSS declarations to style (~200 lines)
- `compute_specificity()` - Calculate selector specificity (~50 lines)

---

### 6. Selector Matching (~150 lines)

**Function**: `matches_selector(&self, element: &Element, selector: &str) -> bool`

**Must Support**:
- Element selectors: `div`, `span`, `p`
- Class selectors: `.container`, `.header`
- ID selectors: `#main`, `#footer`
- Universal selector: `*`
- Descendant combinator: `div span`
- Child combinator: `div > span`
- Adjacent sibling: `div + p`
- General sibling: `div ~ p`
- Attribute selectors: `[type="text"]`
- Pseudo-classes: `:hover`, `:first-child`, `:nth-child(n)`

**macOS Source**: Need to locate
**Windows Target**: rustkit-engine/src/lib.rs (new function)

---

### 7. CSS Property Application (~200 lines)

**Function**: `apply_declarations(&self, style: &mut ComputedStyle, declarations: &[Declaration])`

**Must Parse**: ~70 CSS properties (see progress tracker for full list)

**Current Windows**: Only parses 7 properties in apply_inline_style()
**macOS**: Parses 100+ properties

**Action**: Enhance Windows property parsing to handle all common properties.

---

### 8. Inheritance (~50 lines)

**Inheritable Properties**:
- color
- font-family
- font-size
- font-weight
- font-style
- line-height
- text-align
- text-transform
- letter-spacing
- word-spacing
- white-space
- direction
- visibility

**Function**: `inherit_from_parent(child_style: &mut ComputedStyle, parent_style: &ComputedStyle)`

---

## Testing Gaps

### Windows Missing Tests:
- No tests for stylesheet parsing integration
- No tests for selector matching
- No tests for cascade/specificity
- No end-to-end CSS tests

### macOS Has:
- Stylesheet parsing tests
- Selector matching tests (likely)
- End-to-end tests with real HTML+CSS

---

## macOS Issues to Track

### Potential Issues in macOS (need verification):

1. **Specificity Calculation**: Does macOS correctly calculate specificity for complex selectors?
2. **Cascade Order**: Is source order correctly preserved?
3. **Pseudo-class Support**: Which pseudo-classes are implemented?
4. **Performance**: Is stylesheet matching optimized or naive O(rules * elements)?
5. **Error Handling**: How are malformed CSS rules handled?

**Action**: Review macOS implementation for these issues while porting.

---

## Architecture Notes

### Good Decisions in macOS:
- Stylesheet stored separately from DOM (clean separation)
- Rule matching happens during style computation (correct timing)
- Uses rustkit-cssparser for low-level parsing (reusable)

### Potential Improvements:
- Consider caching matched rules per element (performance)
- Consider style invalidation on DOM changes (correctness)
- Consider CSS variables support (modern web)

---

## Next Steps

1. Find exact location of extract_stylesheets() in macOS
2. Find exact location of matches_selector() in macOS
3. Find exact location of apply_rule() in macOS
4. Port these functions to Windows
5. Test incrementally

---

**Last Updated**: January 16, 2026
