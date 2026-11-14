// Tooltip of a label that may be cut off, reachable without a mouse: it
// shows after a short hover, on keyboard focus and on a touch held for
// LONG_PRESS_MS. One fixed element serves the whole page and moves by
// transform, so showing it never moves layout. The element carries its
// text in data-tip (the layout spec finds holders by it) and points
// aria-describedby at the tooltip while it shows.

import type { Action } from 'svelte/action';

export const HOVER_MS = 400;
export const LONG_PRESS_MS = 500;
/** A tooltip opened by touch stays this long after the finger lifts. */
export const TOUCH_LINGER_MS = 2500;
const ID = 'faderz-tip';
const MARGIN = 6;

let box: HTMLDivElement | null = null;
let owner: HTMLElement | null = null;

function tipBox(): HTMLDivElement {
  if (box?.isConnected) return box;
  box = document.createElement('div');
  box.id = ID;
  box.className = 'tip';
  box.setAttribute('role', 'tooltip');
  box.hidden = true;
  document.body.append(box);
  return box;
}

function show(node: HTMLElement): void {
  const text = node.dataset['tip'];
  if (!text) return;
  const b = tipBox();
  owner?.removeAttribute('aria-describedby');
  owner = node;
  b.textContent = text;
  b.hidden = false;
  const r = node.getBoundingClientRect();
  const t = b.getBoundingClientRect();
  const x = Math.max(MARGIN, Math.min(innerWidth - t.width - MARGIN, r.left + r.width / 2 - t.width / 2));
  const below = r.bottom + MARGIN + t.height <= innerHeight;
  const y = below ? r.bottom + MARGIN : Math.max(MARGIN, r.top - MARGIN - t.height);
  b.style.transform = `translate(${Math.round(x)}px, ${Math.round(y)}px)`;
  node.setAttribute('aria-describedby', ID);
}

function hide(node: HTMLElement): void {
  if (owner !== node) return;
  owner = null;
  node.removeAttribute('aria-describedby');
  if (box) box.hidden = true;
}

/** `use:tip={text}`: the full text of a label, shown as described above. An empty text shows nothing. */
export const tip: Action<HTMLElement, string> = (node, text) => {
  node.dataset['tip'] = text;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pressed = false;
  const clear = () => {
    if (timer) clearTimeout(timer);
    timer = null;
  };
  const later = (fn: () => void, ms: number) => {
    clear();
    timer = setTimeout(fn, ms);
  };
  const enter = (e: PointerEvent) => {
    if (e.pointerType === 'mouse' || e.pointerType === 'pen') later(() => show(node), HOVER_MS);
  };
  const leave = (e: PointerEvent) => {
    if (e.pointerType === 'touch') return;
    clear();
    hide(node);
  };
  const down = (e: PointerEvent) => {
    if (e.pointerType !== 'touch') {
      clear();
      hide(node);
      return;
    }
    pressed = false;
    later(() => {
      pressed = true;
      show(node);
    }, LONG_PRESS_MS);
  };
  const up = (e: PointerEvent) => {
    if (e.pointerType !== 'touch') return;
    if (pressed) later(() => hide(node), TOUCH_LINGER_MS);
    else clear();
  };
  // A long press reads the label; it does not also press the control.
  const click = (e: MouseEvent) => {
    if (!pressed) return;
    pressed = false;
    e.preventDefault();
    e.stopImmediatePropagation();
  };
  const menu = (e: Event) => {
    if (pressed || timer) e.preventDefault();
  };
  const focus = () => {
    if (node.matches(':focus-visible')) show(node);
  };
  const blur = () => {
    clear();
    hide(node);
  };
  const key = (e: KeyboardEvent) => {
    if (e.key === 'Escape' && owner === node) hide(node);
  };
  node.addEventListener('pointerenter', enter);
  node.addEventListener('pointerleave', leave);
  node.addEventListener('pointerdown', down);
  node.addEventListener('pointerup', up);
  node.addEventListener('pointercancel', up);
  node.addEventListener('click', click, true);
  node.addEventListener('contextmenu', menu);
  node.addEventListener('focus', focus);
  node.addEventListener('blur', blur);
  node.addEventListener('keydown', key);
  return {
    update(next: string) {
      node.dataset['tip'] = next;
      if (owner === node) {
        if (next) show(node);
        else hide(node);
      }
    },
    destroy() {
      clear();
      hide(node);
      node.removeEventListener('pointerenter', enter);
      node.removeEventListener('pointerleave', leave);
      node.removeEventListener('pointerdown', down);
      node.removeEventListener('pointerup', up);
      node.removeEventListener('pointercancel', up);
      node.removeEventListener('click', click, true);
      node.removeEventListener('contextmenu', menu);
      node.removeEventListener('focus', focus);
      node.removeEventListener('blur', blur);
      node.removeEventListener('keydown', key);
    },
  };
};
