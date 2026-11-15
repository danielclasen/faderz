// Mounts the mock's dev controls when the mock server answers; the real
// server has no /__mock routes, so nothing appears against it.

import { mount } from 'svelte';
import MockPanel from './MockPanel.svelte';

export async function mountMockPanel(): Promise<void> {
  const res = await fetch('/__mock/replicas').catch(() => null);
  // A dev server passing /api/v1 to a backend answers with its page.
  if (!res?.ok || !res.headers.get('content-type')?.startsWith('application/json')) return;
  const target = document.createElement('div');
  document.body.appendChild(target);
  mount(MockPanel, { target });
}
