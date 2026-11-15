// A press that acts only after a hold: pointer, Space or Enter held for
// `ms` fires `fire`; letting go earlier cancels. While armed the node has
// the class "arming", for a fill that shows the hold. An `ms` of 0 or less
// fires on the press itself. What fires is the `fire` given when the press
// started: an update during the hold changes the next press only.

export interface HoldOptions {
  ms: number;
  fire: () => void;
}

export function hold(node: HTMLElement, options: HoldOptions) {
  let opts = options;
  let timer: ReturnType<typeof setTimeout> | null = null;
  node.style.setProperty('--hold', `${opts.ms}ms`);
  const press = () => {
    if (timer) return;
    const fire = opts.fire;
    if (opts.ms <= 0) return fire();
    node.classList.add('arming');
    timer = setTimeout(() => {
      timer = null;
      node.classList.remove('arming');
      fire();
    }, opts.ms);
  };
  const lift = () => {
    if (timer) clearTimeout(timer);
    timer = null;
    node.classList.remove('arming');
  };
  const down = (e: PointerEvent) => {
    if (e.button !== 0) return;
    node.setPointerCapture(e.pointerId);
    press();
  };
  const key = (e: KeyboardEvent) => {
    if (e.key !== ' ' && e.key !== 'Enter') return;
    e.preventDefault();
    if (e.type === 'keydown' && !e.repeat) press();
    else if (e.type === 'keyup') lift();
  };
  node.addEventListener('pointerdown', down);
  for (const t of ['pointerup', 'pointercancel', 'lostpointercapture', 'blur'] as const) node.addEventListener(t, lift);
  node.addEventListener('keydown', key);
  node.addEventListener('keyup', key);
  return {
    update(next: HoldOptions) {
      opts = next;
      node.style.setProperty('--hold', `${opts.ms}ms`);
    },
    destroy() {
      lift();
      node.removeEventListener('pointerdown', down);
      for (const t of ['pointerup', 'pointercancel', 'lostpointercapture', 'blur'] as const) node.removeEventListener(t, lift);
      node.removeEventListener('keydown', key);
      node.removeEventListener('keyup', key);
    },
  };
}
