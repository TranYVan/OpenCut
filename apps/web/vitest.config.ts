import { defineConfig } from 'vitest/config'
import viteReact from '@vitejs/plugin-react'

// Separate from vite.config.ts: the Cloudflare plugin there rejects the
// environment options vitest sets, so tests run without it.
export default defineConfig({
  resolve: { tsconfigPaths: true },
  plugins: [viteReact()],
  test: {
    environment: 'jsdom',
    // CI runs these beside cargo builds on shared runners, 5-20x slower
    // than a local run; the default 5 s flaked on the ZIP tests.
    testTimeout: 60_000,
  },
})
