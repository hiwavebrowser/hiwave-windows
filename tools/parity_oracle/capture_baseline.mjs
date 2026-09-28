/**
 * capture_baseline.mjs - Full baseline capture (pixel + computed-style + layout rects)
 * 
 * This captures everything needed for triple-verification:
 * 1. baseline.png - Screenshot
 * 2. computed-styles.json - CSS computed values
 * 3. layout-rects.json - DOMRect for all elements
 */

import { chromium } from 'playwright';
import { dirname, resolve, join } from 'path';
import { fileURLToPath } from 'url';
import { existsSync, mkdirSync, writeFileSync } from 'fs';
import {
  createDeterministicContext,
  getDeterministicLaunchOptions,
  shouldApplyParityResetForHtmlPath,
} from './deterministic.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));

// Key CSS properties to capture for computed-style comparison
const KEY_PROPERTIES = [
  // Box model
  'display', 'box-sizing',
  'width', 'height', 'min-width', 'min-height', 'max-width', 'max-height',
  'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
  'margin-top', 'margin-right', 'margin-bottom', 'margin-left',
  'border-top-width', 'border-right-width', 'border-bottom-width', 'border-left-width',
  
  // Positioning
  'position', 'top', 'right', 'bottom', 'left', 'z-index',
  
  // Flexbox
  'flex-direction', 'flex-wrap', 'justify-content', 'align-items', 'align-content',
  'flex-grow', 'flex-shrink', 'flex-basis', 'align-self',
  
  // Grid
  'grid-template-columns', 'grid-template-rows', 'grid-column', 'grid-row', 'gap',
  
  // Typography
  'font-family', 'font-size', 'font-weight', 'font-style', 'line-height',
  'text-align', 'color', 'text-decoration',
  
  // Background
  'background-color', 'background-image', 'background-size', 'background-position',
  
  // Visual
  'opacity', 'visibility', 'overflow', 'overflow-x', 'overflow-y',
  'border-radius',
  
  // Images
  'object-fit', 'object-position', 'aspect-ratio',
];

/**
 * Capture full baseline for a single HTML file
 * 
 * @param {string} htmlPath - Path to HTML file
 * @param {string} outputDir - Directory to save baseline files
 * @param {number} width - Viewport width
 * @param {number} height - Viewport height
 * @returns {Promise<Object>} Capture results
 */
export async function captureBaseline(htmlPath, outputDir, width, height) {
  const absolutePath = resolve(htmlPath);
  
  if (!existsSync(absolutePath)) {
    throw new Error(`HTML file not found: ${absolutePath}`);
  }
  
  mkdirSync(outputDir, { recursive: true });
  
  const browser = await chromium.launch(getDeterministicLaunchOptions());
  
  try {
    const context = await createDeterministicContext(
      browser,
      width,
      height,
      { applyParityReset: shouldApplyParityResetForHtmlPath(absolutePath) }
    );
    
    const page = await context.newPage();
    
    // Load the page
    const fileUrl = `file://${absolutePath}`;
    await page.goto(fileUrl, { waitUntil: 'networkidle' });
    await page.waitForTimeout(50);  // Animations are frozen; allow layout/fonts to settle
    
    // 1. Capture screenshot
    const screenshotPath = join(outputDir, 'baseline.png');
    await page.screenshot({
      path: screenshotPath,
      type: 'png',
      fullPage: false,
    });
    
    // 2. Extract computed styles and layout rects
    const { styles, rects } = await page.evaluate((properties) => {
      const results = { styles: [], rects: [] };
      
      // THE JOIN KEY. Gates A, B and C all match RustKit boxes to Chrome rects
      // on this string, and the engine mirrors it (P0a-0). Changing its output
      // by one character does not fail loudly — unmatched elements silently
      // drop out of comparison and score as "no geometry error" because they
      // were never compared. `verify_selector_key.mjs` extracts this function
      // and asserts it still reproduces all 1757 committed baseline selectors.
      //
      // In particular `/\\s+/` below matches a literal backslash followed by
      // `s`, NOT whitespace, so the split is a no-op and the raw className
      // survives: `div.card featured`, space intact. It reads as a typo and it
      // is what 305 committed selectors say. Do not tidy it without
      // regenerating every baseline and re-mirroring the engine side.
      function getSelector(el) {
        if (el.id) return `#${el.id}`;
        
        const path = [];
        let current = el;
        
        while (current && current.nodeType === 1) {
          let sel = current.tagName.toLowerCase();
          
          if (current.className && typeof current.className === 'string') {
            const classes = current.className.trim().split(/\\s+/).filter(c => c);
            if (classes.length > 0) {
              sel += '.' + classes.slice(0, 2).join('.');
            }
          }
          
          const parent = current.parentElement;
          if (parent) {
            const siblings = Array.from(parent.children).filter(
              c => c.tagName === current.tagName
            );
            if (siblings.length > 1) {
              const idx = siblings.indexOf(current) + 1;
              sel += `:nth-of-type(${idx})`;
            }
          }
          
          path.unshift(sel);
          current = current.parentElement;
          
          if (current && current.tagName === 'BODY') {
            path.unshift('body');
            break;
          }
        }
        
        return path.join(' > ');
      }
      
      const elements = document.querySelectorAll('*');
      
      for (const el of elements) {
        const rect = el.getBoundingClientRect();
        
        // Skip invisible/zero-size elements
        if (rect.width === 0 && rect.height === 0) continue;
        
        // Skip script/style/meta
        const tag = el.tagName.toLowerCase();
        if (['script', 'style', 'meta', 'link', 'head', 'title', 'html'].includes(tag)) continue;
        
        const selector = getSelector(el);
        const computed = getComputedStyle(el);
        
        // Computed styles
        const styleObj = {};
        for (const prop of properties) {
          styleObj[prop] = computed.getPropertyValue(prop);
        }
        
        results.styles.push({
          selector,
          tag,
          id: el.id || null,
          className: el.className || null,
          styles: styleObj,
        });
        
        // Layout rect
        results.rects.push({
          selector,
          tag,
          rect: {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
            left: rect.left,
          },
          client: {
            width: el.clientWidth,
            height: el.clientHeight,
          },
          scroll: {
            width: el.scrollWidth,
            height: el.scrollHeight,
            top: el.scrollTop,
            left: el.scrollLeft,
          },
        });
      }
      
      return results;
    }, KEY_PROPERTIES);
    
    // Save computed styles
    const stylesPath = join(outputDir, 'computed-styles.json');
    writeFileSync(stylesPath, JSON.stringify({
      timestamp: new Date().toISOString(),
      viewport: { width, height },
      elementCount: styles.length,
      elements: styles,
    }, null, 2));
    
    // Save layout rects
    const rectsPath = join(outputDir, 'layout-rects.json');
    writeFileSync(rectsPath, JSON.stringify({
      timestamp: new Date().toISOString(),
      viewport: { width, height },
      elementCount: rects.length,
      elements: rects,
    }, null, 2));
    
    await context.close();
    
    return {
      success: true,
      screenshot: screenshotPath,
      styles: stylesPath,
      rects: rectsPath,
      elementCount: styles.length,
    };
    
  } finally {
    await browser.close();
  }
}

/**
 * Capture baselines for multiple cases
 * 
 * @param {Array<{id: string, htmlPath: string, width: number, height: number}>} cases
 * @param {string} baseOutputDir - Base directory for baselines
 * @returns {Promise<Object>} Results keyed by case ID
 */
export async function captureMultipleBaselines(cases, baseOutputDir) {
  const results = {};
  
  for (const caseInfo of cases) {
    const { id, htmlPath, width, height } = caseInfo;
    const outputDir = join(baseOutputDir, id);
    
    console.log(`  Capturing ${id}...`);
    
    try {
      const result = await captureBaseline(htmlPath, outputDir, width, height);
      results[id] = result;
      console.log(`    OK (${result.elementCount} elements)`);
    } catch (err) {
      results[id] = { success: false, error: err.message };
      console.log(`    FAIL: ${err.message}`);
    }
  }
  
  return results;
}



