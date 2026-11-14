import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig(async ({ command, mode }) => ({
  plugins: [
    svelte(),
    // The mock server exists only in `vite` dev serving; builds never load it.
    ...(command === 'serve' && mode !== 'test' ? [(await import('./dev/mock/plugin.ts')).faderzMock()] : []),
  ],
  server: {
    fs: { allow: ['..'] },
  },
  build: {
    target: 'es2022',
    sourcemap: true,
  },
  test: {
    include: ['tests/unit/**/*.test.ts', 'tests/contract/**/*.test.ts'],
    environment: 'node',
  },
}));
