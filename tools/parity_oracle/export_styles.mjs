/**
 * export_styles.mjs - Export computed styles from Chrome for CSS debugging
 */

import { chromium } from 'playwright';
import { resolve } from 'path';
import { existsSync } from 'fs';
import {
  createDeterministicContext,
  getDeterministicLaunchOptions,
  shouldApplyParityResetForHtmlPath,
} from './deterministic.mjs';

// Key CSS properties to extract for comparison
const KEY_PROPERTIES = [
  // Box model
  'display',
  'width',
  'height',
  'min-width',
  'min-height',
  'max-width',
  'max-height',
  'padding-top',
  'padding-right',
  'padding-bottom',
  'padding-left',
  'margin-top',
  'margin-right',
  'margin-bottom',
  'margin-left',
  'border-top-width',
  'border-right-width',
  'border-bottom-width',
  'border-left-width',
  'box-sizing',
  
  // Positioning
  'position',
  'top',
  'right',
  'bottom',
  'left',
  'z-index',
  
  // Flexbox
  'flex-direction',
  'flex-wrap',
  'justify-content',
  'align-items',
  'align-content',
  'flex-grow',
  'flex-shrink',
  'flex-basis',
  'align-self',
  
  // Grid
  'grid-template-columns',
  'grid-template-rows',
  'grid-column',
  'grid-row',
  'gap',
  
  // Typography
  'font-family',
  'font-size',
  'font-weight',
  'font-style',
  'line-height',
  'text-align',
  'color',
  
  // Background
  'background-color',
  'background-image',
  
  // Visual
  'opacity',
  'visibility',
  'overflow',
  'overflow-x',
  'overflow-y',
  
  // Transform
  'transform',
  'transform-origin',
];

/**
 * Generate a unique CSS selector for an element
 * 
 * @param {Element} el - DOM element
 * @returns {string} Unique selector
 */
function getUniqueSelector(el) {
  if (el.id) {
    return `#${el.id}`;
  }
  
  const path = [];
  let current = el;
  
  while (current && current.nodeType === 1) {
    let selector = current.tagName.toLowerCase();
    
    if (current.className && typeof current.className === 'string') {
      const classes = current.className.trim().split(/\s+/).filter(c => c);
      if (classes.length > 0) {
        selector += '.' + classes.slice(0, 2).join('.');
      }
    }
    
    // Add nth-child for disambiguation
    const parent = current.parentElement;
    if (parent) {
      const siblings = Array.from(parent.children).filter(
        c => c.tagName === current.tagName
      );
      if (siblings.length > 1) {
        const idx = siblings.indexOf(current) + 1;
        selector += `:nth-of-type(${idx})`;
      }
    }
    
    path.unshift(selector);
    current = current.parentElement;
    
    // Stop at body
    if (current && current.tagName === 'BODY') {
      path.unshift('body');
      break;
    }
  }
  
  return path.join(' > ');
}

/**
 * Export computed styles for all elements in a page
 * 
 * @param {string} htmlPath - Path to HTML file
 * @param {number} width - Viewport width
 * @param {number} height - Viewport height
 * @param {string} selector - CSS selector to limit elements (default: all visible)
 * @returns {Promise<Array>} Array of element style objects
 */
export async function exportStyles(htmlPath, width, height, selector = '*') {
  const absolutePath = resolve(htmlPath);
  
  if (!existsSync(absolutePath)) {
    throw new Error(`HTML file not found: ${absolutePath}`);
  }
  
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
    await page.waitForTimeout(50);
    
    // Extract styles
    const styles = await page.evaluate((args) => {
      const { selector, properties } = args;
      const elements = document.querySelectorAll(selector);
      const results = [];
      
      // Helper to get unique selector
      function getSelector(el) {
        if (el.id) return `#${el.id}`;
        
        const path = [];
        let current = el;
        
        while (current && current.nodeType === 1) {
          let sel = current.tagName.toLowerCase();
          
          if (current.className && typeof current.className === 'string') {
            const classes = current.className.trim().split(/\s+/).filter(c => c);
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
      
      for (const el of elements) {
        // Skip invisible elements
        const rect = el.getBoundingClientRect();
        if (rect.width === 0 && rect.height === 0) continue;
        
        // Skip script/style/meta tags
        const tag = el.tagName.toLowerCase();
        if (['script', 'style', 'meta', 'link', 'head', 'title'].includes(tag)) continue;
        
        const computed = getComputedStyle(el);
        const styleObj = {};
        
        for (const prop of properties) {
          styleObj[prop] = computed.getPropertyValue(prop);
        }
        
        results.push({
          selector: getSelector(el),
          tag: tag,
          id: el.id || null,
          className: el.className || null,
          rect: {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
          },
          styles: styleObj,
        });
      }
      
      return results;
    }, { selector, properties: KEY_PROPERTIES });
    
    await context.close();
    return styles;
    
  } finally {
    await browser.close();
  }
}

/**
 * Compare computed styles between Chrome and RustKit
 * 
 * @param {Array} chromeStyles - Styles from Chrome
 * @param {Array} rustkitStyles - Styles from RustKit
 * @returns {Object} Comparison results
 */
export function compareStyles(chromeStyles, rustkitStyles) {
  const chromeMap = new Map(chromeStyles.map(s => [s.selector, s]));
  const rustkitMap = new Map(rustkitStyles.map(s => [s.selector, s]));
  
  const results = {
    matched: 0,
    mismatched: 0,
    chromeOnly: 0,
    rustkitOnly: 0,
    differences: [],
  };
  
  // Compare elements found in both
  for (const [selector, chrome] of chromeMap) {
    const rustkit = rustkitMap.get(selector);
    
    if (!rustkit) {
      results.chromeOnly++;
      continue;
    }
    
    const diffs = [];
    for (const prop of KEY_PROPERTIES) {
      const chromeVal = chrome.styles[prop];
      const rustkitVal = rustkit.styles?.[prop];
      
      if (chromeVal !== rustkitVal) {
        diffs.push({
          property: prop,
          chrome: chromeVal,
          rustkit: rustkitVal,
        });
      }
    }
    
    if (diffs.length > 0) {
      results.mismatched++;
      results.differences.push({
        selector,
        tag: chrome.tag,
        propertyDiffs: diffs,
      });
    } else {
      results.matched++;
    }
  }
  
  // Count RustKit-only elements
  for (const selector of rustkitMap.keys()) {
    if (!chromeMap.has(selector)) {
      results.rustkitOnly++;
    }
  }
  
  return results;
}



