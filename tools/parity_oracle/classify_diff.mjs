/**
 * classify_diff.mjs
 *
 * Best-effort taxonomy classification for diff contributors.
 * This is intentionally heuristic; the goal is triage, not perfect labeling.
 */

function isNonZeroBorderRadius(val) {
  if (!val) return false;
  const s = String(val).trim();
  return s !== '' && s !== '0px' && s !== '0px 0px' && s !== '0px 0px 0px 0px' && s !== '0';
}

export function classifyContributor(contrib, chromeStyleBySelector) {
  const styles = chromeStyleBySelector?.get(contrib.selector)?.styles || {};
  const tag = (contrib.tag || '').toLowerCase();

  // Replaced elements / platform widgets.
  if (['img', 'video', 'canvas', 'svg'].includes(tag)) return 'replaced_content';
  if (['input', 'button', 'select', 'textarea'].includes(tag)) return 'form_control';

  const bgImg = String(styles['background-image'] || styles['background'] || '');
  if (bgImg.includes('gradient')) return 'gradient_interpolation';

  const br = styles['border-radius'];
  if (isNonZeroBorderRadius(br) && (contrib.corner_ratio || 0) >= 0.35) return 'clip_radius';

  const color = styles['color'];
  const fontFamily = styles['font-family'];
  const fontSize = styles['font-size'];
  if (color && fontFamily && fontSize) return 'text_metrics';

  const bgColor = styles['background-color'];
  if (bgColor && bgColor.trim() !== '' && bgColor.trim() !== 'rgba(0, 0, 0, 0)') return 'paint_solid';

  return 'unknown';
}

export function buildChromeStyleIndex(chromeStylesJson) {
  const elements = chromeStylesJson?.elements || [];
  return new Map(elements.map((e) => [e.selector, e]));
}


