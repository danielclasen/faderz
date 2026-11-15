import { defineConfig } from 'vitest/config';

// The contract tests of the faderz backend: FADERZ_BIN names the binary.
export default defineConfig({
  test: {
    include: ['tests/backend/**/*.test.ts'],
    environment: 'node',
    testTimeout: 20_000,
    hookTimeout: 30_000,
  },
});
