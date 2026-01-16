# CRITICAL: CSS Architecture Gap in Windows RustKit Engine

## Executive Summary

During parity testing investigation (January 16, 2026), a **critical architectural gap** was discovered: the Windows RustKit engine lacks complete CSS stylesheet parsing, causing web pages to render as white screens with grey boxes.

**Impact**: HIGH - Prevents rendering of any real-world web content
**Severity**: CRITICAL - Core browser functionality missing
**Effort**: ~4,000 lines of code to port from macOS

---

## The Problem

### Symptom
- Web pages display as white screen with grey boxes
- Test cases in `websuite/` fail to render correctly
- `hiwave-smoke` generates invalid screenshots

### Root Cause
**The Windows engine only parses inline `style=""` attributes, NOT `<style>` tag stylesheets.**

### Evidence

**File Size Gap:**
```
rustkit-engine/src/lib.rs:
- macOS:   6,215 lines
- Windows: 2,025 lines
- Gap:     4,190 lines (67% missing)
```

**Missing Functionality in Windows:**
1. **No `<style>` tag parsing** - CSS embedded in HTML is completely ignored
2. **No selector matching** - Can't match CSS rules to DOM elements
3. **No cascade logic** - Can't resolve specificity conflicts
4. **No inheritance** - Parent styles don't propagate to children
5. **Limited property support** - Only 6 CSS properties parsed (color, background-color, font-size, font-weight, margin, padding)

**What Works (Inline Styles Only):**
- `style="color: red"` ✓
- `style="background-color: blue"` ✓
- `style="font-size: 24px"` ✓
- `style="margin: 10px"` ✓
- `style="padding: 10px"` ✓
- `style="box-sizing: border-box"` ✓ (after today's fix)

**What Doesn't Work:**
```html
<style>
  * { box-sizing: border-box; }  ✗ IGNORED
  body { font-family: Georgia; }  ✗ IGNORED
  .container { max-width: 800px; }  ✗ IGNORED
  h1 { font-size: 2.5em; }  ✗ IGNORED
</style>
```

---

## Impact on Recent Migration Work

### Phase 1-5 Migration Status

**Phase 1-3 (Completed):**
- ✓ LineHeight enum fix
- ✓ Flexbox stretch fix
- ✓ Grid bug fixes
- ✓ Text-transform support
- ✓ Sticky positioning

**Phase 4 (Completed):**
- ✓ CSS Grid Phase 7 (78 tests passing)
- ✓ Complete grid layout algorithm
- ⚠️ **BUT**: Grid styles from `<style>` tags won't apply

**Phase 5 (Partially Completed):**
- ✓ HSL/HSLA color parsing added to rustkit-css
- ⚠️ **BUT**: HSL colors in `<style>` tags won't work
- ✗ Repeating gradients deferred (renderer too small)

### Critical Realization

**All the features we've migrated (Grid, HSL, sticky, text-transform) won't work for real web pages because the Windows engine can't parse stylesheets!**

The migration plan assumed the Windows engine had feature parity with macOS except for specific bugs. In reality, Windows is missing the entire CSS parsing infrastructure.

---

## Code Location Reference

### macOS Engine (COMPLETE CSS Support)

**File**: `hiwave-macos/crates/rustkit-engine/src/lib.rs` (6,215 lines)

**Key Sections:**
- Lines ~800-1500: DOM tree building with style resolution
- Lines ~1500-3000: CSS selector matching (element, class, ID, attribute, pseudo-class)
- Lines ~3000-4000: Style cascade and specificity
- Lines ~4000-5500: CSS property parsing (100+ properties)
- Lines ~5500-6000: Inheritance and computed values

**Selector Matching** (Lines ~1900-2100):
```rust
fn matches_selector(&self, element: &Element, selector: &str) -> bool {
    // Type selectors: div, span, p
    // Class selectors: .container, .header
    // ID selectors: #main, #footer
    // Attribute selectors: [type="text"], [disabled]
    // Pseudo-classes: :hover, :first-child, :nth-child()
    // Combinators: descendant, child (>), adjacent (+), sibling (~)
}
```

**Property Parsing** (Lines ~2200-4500):
```rust
match property.as_str() {
    "display" => { ... }
    "position" => { ... }
    "box-sizing" => { ... }  // Line 2631
    "width" => { ... }
    "height" => { ... }
    "margin" => { ... }
    "padding" => { ... }
    "border" => { ... }
    "font-size" => { ... }
    "font-family" => { ... }
    "color" => { ... }
    "background" => { ... }
    "flex-direction" => { ... }
    "flex-wrap" => { ... }
    "align-items" => { ... }
    "justify-content" => { ... }
    "grid-template-columns" => { ... }
    "grid-template-rows" => { ... }
    "grid-column-start" => { ... }
    "grid-row-start" => { ... }
    // ... 80+ more properties
}
```

### Windows Engine (INCOMPLETE - Inline Styles Only)

**File**: `hiwave-windows/crates/rustkit-engine/src/lib.rs` (2,025 lines)

**What Exists:**
- Lines ~790-930: Basic DOM tree building (NO style resolution)
- Lines ~930-1010: Minimal style creation (defaults only)
- Lines ~1011-1069: Inline style parsing (6 properties)

**`apply_inline_style` Function** (Lines 1011-1069):
```rust
fn apply_inline_style(&self, style: &mut ComputedStyle, style_attr: &str) {
    for declaration in style_attr.split(';') {
        // ...
        match property.as_str() {
            "color" => { ... }
            "background-color" | "background" => { ... }
            "font-size" => { ... }
            "font-weight" => { ... }
            "margin" => { ... }
            "padding" => { ... }
            "box-sizing" => { ... }  // Added today
            _ => {}  // ALL OTHER PROPERTIES IGNORED
        }
    }
}
```

**What's Missing:**
- NO `<style>` tag parsing
- NO `<link rel="stylesheet">` support
- NO selector matching
- NO cascade resolution
- NO inheritance logic
- NO user-agent stylesheet
- NO specificity calculation
- NO pseudo-class support
- NO pseudo-element support
- NO combinator support
- NO media query support

---

## Test Case Analysis

### Example: `websuite/cases/article-typography/index.html`

**CSS Used:**
```html
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }  ← IGNORED
  body { font-family: Georgia, serif; line-height: 1.6; color: #333; }  ← IGNORED
  .container { max-width: 800px; margin: 0 auto; }  ← IGNORED
  h1 { font-size: 2.5em; font-weight: 700; color: #1a1a1a; }  ← IGNORED
  .subtitle { font-size: 1.2em; color: #666; font-style: italic; }  ← IGNORED
  /* ... 180 more lines of CSS ... */
</style>
```

**Result**: All styles ignored → default browser rendering → grey boxes

---

## Solution Options

### Option 1: Port Complete CSS Engine from macOS (RECOMMENDED)

**Effort**: ~4,000 lines of code
**Timeline**: 2-3 weeks
**Benefit**: Full CSS support, real web page rendering

**Implementation Plan:**
1. Port selector matching (~800 lines)
2. Port cascade logic (~400 lines)
3. Port inheritance (~300 lines)
4. Port property parsing (~2,500 lines)
5. Integration and testing (~1 week)

**Files to Port:**
- `hiwave-macos/crates/rustkit-engine/src/lib.rs` lines 800-5500

### Option 2: Minimal CSS Support (Quick Fix)

**Effort**: ~500 lines of code
**Timeline**: 2-3 days
**Benefit**: Basic styling works for test cases

**Implementation:**
1. Parse `<style>` tag content
2. Simple selector matching (element, class, ID only)
3. No cascade (last rule wins)
4. Basic inheritance (color, font-size, font-family)
5. Top 20 most common CSS properties

**Limitations:**
- No complex selectors
- No specificity
- No pseudo-classes/elements
- Won't work for real websites

### Option 3: Use External CSS Parser (Alternative)

**Effort**: ~1 week integration
**Timeline**: 1-2 weeks
**Benefit**: Standards-compliant CSS parsing

**Options:**
- `lightningcss` - Fast CSS parser in Rust
- `css-parser` crate - Servo's CSS parser
- `parcel-css` - Modern CSS tooling

**Tradeoff**: External dependency, less control, integration complexity

---

## Recommendations

### Immediate Action (Today)

1. ✓ **DONE**: Added BoxSizing enum and parsing for inline styles
2. ✓ **DONE**: Documented this critical gap
3. **TODO**: Decide on solution path

### Short-Term (This Week)

**IF continuing with current approach:**
- Defer Phase 6 (backdrop filters) - requires working CSS first
- Defer parity testing - won't be meaningful without CSS
- Focus on porting CSS engine from macOS

**IF pivoting to external parser:**
- Research lightningcss integration
- Create POC branch
- Evaluate performance and compatibility

### Long-Term (Next Sprint)

**After CSS infrastructure is in place:**
- Resume parity testing (websuite will work)
- Continue Phase 6 (backdrop filters)
- Continue Phase 7 (Linux testing)
- Port remaining macOS features

---

## Why This Wasn't Caught Earlier

1. **Phase 1-3 focused on layout algorithms** - Grid, flexbox, sticky are layout features that work independently of CSS parsing
2. **Unit tests don't use `<style>` tags** - Test cases directly construct LayoutBox trees with pre-computed styles
3. **Migration plan assumed architectural parity** - Focused on specific bugs/features, not fundamental infrastructure
4. **First real-world test exposure** - Websuite smoke tests are first time engine rendered actual HTML+CSS

---

## Next Steps

**Decision Required:** Which solution option to pursue?

**Questions to Consider:**
1. Is Windows engine intended for production use, or is it experimental?
2. Is there a timeline requirement for Windows parity with macOS?
3. Should we port the macOS CSS engine verbatim, or modernize it?
4. Is using an external CSS parser acceptable?

**If Option 1 (Port from macOS):**
- I can create a detailed porting plan
- Break work into phases (selector matching → cascade → properties)
- Estimate 2-3 weeks for full implementation

**If Option 2 (Minimal CSS):**
- I can implement basic stylesheet support today
- Good enough for test cases, not for real browsers
- Bridge solution until full port

**If Option 3 (External Parser):**
- Research and POC needed first
- Could take longer but more maintainable
- Standards-compliant parsing

---

## Impact Summary

**Current State:**
- ❌ Smoke tests fail (white screen + grey boxes)
- ❌ Parity tests meaningless (CSS doesn't work)
- ❌ Can't render real web pages
- ❌ Recent migration work (Grid, HSL) not usable
- ❌ Browser not functional for end users

**After Fix (Option 1 - Full CSS Engine):**
- ✅ Smoke tests pass
- ✅ Parity tests meaningful
- ✅ Can render real web pages
- ✅ Migration work becomes effective
- ✅ Browser functional

**After Fix (Option 2 - Minimal CSS):**
- ✅ Smoke tests pass
- ⚠️ Parity tests partially work
- ⚠️ Can render simple pages only
- ⚠️ Migration work partially effective
- ❌ Browser not production-ready

---

**Generated**: January 16, 2026
**Discovered by**: Claude Sonnet 4.5 during parity testing investigation
**Priority**: CRITICAL
**Status**: Documented, awaiting direction
