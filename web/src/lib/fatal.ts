// Last-resort error display: replaces the page with the error text, so a
// failure during start-up shows its cause instead of a blank page.

export function renderFatal(error: unknown, target: HTMLElement = document.body): void {
  const text = error instanceof Error ? `${error.name}: ${error.message}\n\n${error.stack ?? ''}` : String(error);
  const box = document.createElement('div');
  box.setAttribute('role', 'alert');
  box.dataset['testid'] = 'fatal';
  box.style.cssText =
    'position:fixed;inset:0;z-index:1000;overflow:auto;padding:16px;background:#0b0d10;color:#ffb4b8;' +
    'font:13px/1.45 ui-monospace,monospace;white-space:pre-wrap;word-break:break-word';
  const title = document.createElement('strong');
  title.textContent = 'faderz failed to start\n\n';
  box.append(title, document.createTextNode(text));
  target.replaceChildren(box);
}
