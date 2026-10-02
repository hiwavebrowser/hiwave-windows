/**
 * compare_pixels.mjs - Pixel comparison using pixelmatch
 */

import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';
import { readFileSync, writeFileSync, existsSync } from 'fs';

/**
 * Convert PPM (P6 binary) to RGBA buffer
 * 
 * @param {string} ppmPath - Path to PPM file
 * @returns {{width: number, height: number, data: Buffer}} RGBA image data
 */
export function ppmToRgba(ppmPath) {
  const buffer = readFileSync(ppmPath);
  
  // Parse PPM header (P6 format)
  let idx = 0;
  
  // Skip magic number "P6"
  while (buffer[idx] !== 0x0A) idx++;
  idx++;
  
  // Skip comments
  while (buffer[idx] === 0x23) { // '#'
    while (buffer[idx] !== 0x0A) idx++;
    idx++;
  }
  
  // Read width
  let widthStr = '';
  while (buffer[idx] !== 0x20 && buffer[idx] !== 0x0A) {
    widthStr += String.fromCharCode(buffer[idx]);
    idx++;
  }
  idx++; // Skip space/newline
  
  // Read height
  let heightStr = '';
  while (buffer[idx] !== 0x20 && buffer[idx] !== 0x0A) {
    heightStr += String.fromCharCode(buffer[idx]);
    idx++;
  }
  idx++; // Skip space/newline
  
  // Read max value (usually 255)
  let maxValStr = '';
  while (buffer[idx] !== 0x0A) {
    maxValStr += String.fromCharCode(buffer[idx]);
    idx++;
  }
  idx++; // Skip newline
  
  const width = parseInt(widthStr, 10);
  const height = parseInt(heightStr, 10);
  const maxVal = parseInt(maxValStr, 10);
  
  if (isNaN(width) || isNaN(height) || isNaN(maxVal)) {
    throw new Error(`Invalid PPM header: width=${widthStr}, height=${heightStr}, maxVal=${maxValStr}`);
  }
  
  // Convert RGB to RGBA
  const rgbaData = Buffer.alloc(width * height * 4);
  const pixelCount = width * height;
  
  for (let i = 0; i < pixelCount; i++) {
    const srcIdx = idx + i * 3;
    const dstIdx = i * 4;
    
    rgbaData[dstIdx] = buffer[srcIdx];       // R
    rgbaData[dstIdx + 1] = buffer[srcIdx + 1]; // G
    rgbaData[dstIdx + 2] = buffer[srcIdx + 2]; // B
    rgbaData[dstIdx + 3] = 255;                // A (fully opaque)
  }
  
  return { width, height, data: rgbaData };
}

/**
 * Load a PNG file as RGBA buffer
 * 
 * @param {string} pngPath - Path to PNG file
 * @returns {Promise<{width: number, height: number, data: Buffer}>}
 */
export async function loadPng(pngPath) {
  return new Promise((resolve, reject) => {
    const buffer = readFileSync(pngPath);
    const png = new PNG();
    
    png.parse(buffer, (err, data) => {
      if (err) {
        reject(err);
      } else {
        resolve({
          width: data.width,
          height: data.height,
          data: data.data,
        });
      }
    });
  });
}

/**
 * Save RGBA buffer as PNG
 * 
 * @param {Buffer} data - RGBA buffer
 * @param {number} width
 * @param {number} height
 * @param {string} outputPath
 */
export function savePng(data, width, height, outputPath) {
  const png = new PNG({ width, height });
  png.data = data;
  
  const buffer = PNG.sync.write(png);
  writeFileSync(outputPath, buffer);
}

/**
 * Compare two images and produce a diff
 * 
 * @param {string} chromePath - Path to Chrome baseline (PNG)
 * @param {string} rustkitPath - Path to RustKit capture (PPM or PNG)
 * @param {string} diffPath - Path to save diff image (PNG)
 * @param {Object} options - pixelmatch options
 * @returns {Promise<{diffPixels: number, totalPixels: number, diffPercent: number}>}
 */
export async function comparePixels(chromePath, rustkitPath, diffPath, options = {}) {
  // Load Chrome baseline
  const chrome = await loadPng(chromePath);
  
  // Load RustKit capture (detect format)
  let rustkit;
  if (rustkitPath.endsWith('.ppm')) {
    rustkit = ppmToRgba(rustkitPath);
  } else {
    rustkit = await loadPng(rustkitPath);
  }
  
  // Dimension mismatch is an INSTRUMENT FAILURE, never a soft crop
  // (measurement lie #8 class). Score 100 unless RK_ALLOW_CROP=1.
  if (chrome.width !== rustkit.width || chrome.height !== rustkit.height) {
    const msg = `Chrome ${chrome.width}x${chrome.height} vs RustKit ${rustkit.width}x${rustkit.height}`;
    if (process.env.RK_ALLOW_CROP !== '1') {
      console.error(
        `INSTRUMENT FAILURE — dimension mismatch: ${msg} (score 100, not cropped; RK_ALLOW_CROP=1 to debug)`
      );
      return {
        diffPixels: chrome.width * chrome.height,
        totalPixels: chrome.width * chrome.height,
        diffPercent: 100,
        width: chrome.width,
        height: chrome.height,
        instrumentFailure: `dimension_mismatch: ${msg}`,
      };
    }
    console.warn(`Dimension mismatch (RK_ALLOW_CROP=1): ${msg}`);

    // Use smaller dimensions
    const width = Math.min(chrome.width, rustkit.width);
    const height = Math.min(chrome.height, rustkit.height);

    // Crop both to same size (top-left corner)
    const cropChrome = cropImage(chrome.data, chrome.width, chrome.height, width, height);
    const cropRustkit = cropImage(rustkit.data, rustkit.width, rustkit.height, width, height);

    chrome.data = cropChrome;
    chrome.width = width;
    chrome.height = height;
    rustkit.data = cropRustkit;
    rustkit.width = width;
    rustkit.height = height;
  }
  
  const { width, height } = chrome;
  const totalPixels = width * height;
  
  // Create diff image buffer
  const diffData = Buffer.alloc(width * height * 4);
  
  // Run pixelmatch
  const diffPixels = pixelmatch(
    chrome.data,
    rustkit.data,
    diffData,
    width,
    height,
    {
      threshold: 0.1,  // Sensitivity (0 = exact match, 1 = ignore all)
      includeAA: true, // Include anti-aliased pixels
      alpha: 0.1,      // Blending for diff visualization
      ...options,
    }
  );
  
  // Save diff image
  if (diffPath) {
    savePng(diffData, width, height, diffPath);
  }
  
  const diffPercent = (diffPixels / totalPixels) * 100;
  
  return {
    diffPixels,
    totalPixels,
    diffPercent,
    width,
    height,
  };
}

/**
 * Crop an RGBA image to specified dimensions (top-left origin)
 */
function cropImage(data, srcWidth, srcHeight, dstWidth, dstHeight) {
  const cropped = Buffer.alloc(dstWidth * dstHeight * 4);
  
  for (let y = 0; y < dstHeight; y++) {
    for (let x = 0; x < dstWidth; x++) {
      const srcIdx = (y * srcWidth + x) * 4;
      const dstIdx = (y * dstWidth + x) * 4;
      
      cropped[dstIdx] = data[srcIdx];
      cropped[dstIdx + 1] = data[srcIdx + 1];
      cropped[dstIdx + 2] = data[srcIdx + 2];
      cropped[dstIdx + 3] = data[srcIdx + 3];
    }
  }
  
  return cropped;
}

/**
 * Generate a heatmap showing intensity of differences
 * 
 * @param {Buffer} diffData - Diff image RGBA buffer
 * @param {number} width
 * @param {number} height
 * @param {string} outputPath
 */
export function generateHeatmap(diffData, width, height, outputPath) {
  const heatmap = Buffer.alloc(width * height * 4);
  
  for (let i = 0; i < width * height; i++) {
    const srcIdx = i * 4;
    const r = diffData[srcIdx];
    const g = diffData[srcIdx + 1];
    const b = diffData[srcIdx + 2];
    
    // Calculate intensity (how different)
    const intensity = Math.max(r, g, b);
    
    // Map to heatmap colors (blue -> green -> yellow -> red)
    let hr, hg, hb;
    if (intensity < 64) {
      hr = 0; hg = intensity * 4; hb = 255 - intensity * 4;
    } else if (intensity < 128) {
      hr = (intensity - 64) * 4; hg = 255; hb = 0;
    } else if (intensity < 192) {
      hr = 255; hg = 255 - (intensity - 128) * 4; hb = 0;
    } else {
      hr = 255; hg = 0; hb = (intensity - 192) * 4;
    }
    
    heatmap[srcIdx] = hr;
    heatmap[srcIdx + 1] = hg;
    heatmap[srcIdx + 2] = hb;
    heatmap[srcIdx + 3] = intensity > 0 ? 255 : 0;
  }
  
  savePng(heatmap, width, height, outputPath);
}



