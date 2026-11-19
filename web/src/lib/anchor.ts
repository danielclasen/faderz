// Where a panel that opens from a key goes: fixed to the viewport, above
// or below the key, kept inside the viewport by EDGE px.

/** Least room between a panel and the viewport's edges, px. */
export const EDGE = 8;
/** Room between the key and the panel, px. */
export const GAP = 4;

export interface Placement {
  left: number;
  /** Set for a panel below its key. */
  top?: number;
  /** Set for a panel above its key: from the viewport's bottom edge. */
  bottom?: number;
}

/**
 * The place of a panel `width` px wide beside `key`: `centre` centres it on
 * the key, `start` lines its left edge up with the key's, `end` its right
 * edge with the key's. A panel that would run off the viewport moves in.
 */
export function place(
  key: Pick<DOMRect, 'left' | 'right' | 'top' | 'bottom' | 'width'>,
  width: number,
  view: { width: number; height: number },
  side: 'above' | 'below',
  align: 'start' | 'centre' | 'end',
  /** Below: the panel's top edge goes no higher than this, such as a header's bottom edge. */
  floor = -Infinity,
): Placement {
  const wanted = align === 'start' ? key.left : align === 'end' ? key.right - width : key.left + key.width / 2 - width / 2;
  const left = Math.max(EDGE, Math.min(wanted, view.width - EDGE - width));
  return side === 'below' ? { left, top: Math.max(key.bottom, floor) + GAP } : { left, bottom: view.height - key.top + GAP };
}

/**
 * Keeps `panel` placed beside `key` while it is open: again when the panel
 * resizes, as its content changes, and when the viewport does. A panel
 * below a key in the page's header opens below the whole header, which
 * can have two rows. Returns the function that stops it.
 */
export function follow(
  key: HTMLElement,
  panel: HTMLElement,
  side: 'above' | 'below',
  align: 'start' | 'centre' | 'end',
  set: (p: Placement) => void,
): () => void {
  const update = () => {
    const view = document.documentElement;
    const header = key.closest('header')?.getBoundingClientRect().bottom;
    set(place(key.getBoundingClientRect(), panel.offsetWidth, { width: view.clientWidth, height: view.clientHeight }, side, align, header));
  };
  update();
  const sized = new ResizeObserver(update);
  sized.observe(panel);
  window.addEventListener('resize', update);
  return () => {
    sized.disconnect();
    window.removeEventListener('resize', update);
  };
}
