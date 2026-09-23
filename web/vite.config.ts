import { fileURLToPath } from 'node:url'
import { lingui, linguiTransformerBabelPreset } from '@lingui/vite-plugin'
import babel from '@rolldown/plugin-babel'
import tailwindcss from '@tailwindcss/vite'
import { tanstackRouter } from '@tanstack/router-plugin/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig(({ command }) => ({
  plugins: [
    // Generates src/routeTree.gen.ts from src/routes/. Committed, and CI fails
    // when it is stale (ADR-0008).
    tanstackRouter({ target: 'react', autoCodeSplitting: true }),

    // plugin-react 6 transforms JSX with Oxc and has NO `babel` option — Vite 8
    // is Rolldown-based and Babel is no longer part of this plugin.
    react(),

    // ADR-0011: Lingui macros still need a Babel pass, so it arrives as a
    // separate Rolldown plugin. `linguiTransformerBabelPreset` exists precisely
    // for this wiring. The @lingui/swc-plugin route stays unused: it is
    // officially experimental with no semver guarantee across swc_core
    // versions.
    lingui({
      // A missing translation should fail the build, not fall back at runtime.
      // Only on `build`, so an untranslated string does not block the dev loop.
      failOnMissing: command === 'build',
      failOnCompileError: command === 'build',
    }),
    babel({ presets: [linguiTransformerBabelPreset()] }),

    tailwindcss(),
  ],

  server: {
    proxy: {
      // changeOrigin stays false so the session cookie's origin is preserved
      // (ADR-0005, ADR-0006).
      '/api': { target: 'http://localhost:8080', changeOrigin: false },
    },
  },

  resolve: {
    // Declared here as well as in tsconfig. The build resolved `@/` without
    // it, but Vitest did not — and an alias that works in one and not the
    // other is a test suite that cannot import what the application imports.
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },

  test: {
    // Component tests need a DOM. Pure-logic tests run in the same
    // environment rather than a second config: one place to look, and the
    // cost of jsdom for a handful of logic tests is smaller than the cost of
    // wondering which config a test ran under.
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    // No implicit globals. An imported `describe` is one a reader can follow
    // to its definition, and it keeps the test files valid TypeScript without
    // a types entry that has to be kept in step.
    globals: false,
    // Excluded because Playwright owns them and Vitest would try to run them.
    exclude: ['node_modules/**', 'e2e/**', 'dist/**'],
  },

  build: {
    target: 'es2022',
    rollupOptions: {
      output: {
        // Rolldown replaced the object form of `manualChunks` with groups of
        // { name, test } pairs. Note `advancedChunks` is the deprecated
        // spelling; `codeSplitting` is current and takes priority if both are
        // set.
        codeSplitting: {
          groups: [
            { name: 'router', test: /node_modules[\\/]@tanstack[\\/]react-router/ },
            { name: 'query', test: /node_modules[\\/]@tanstack[\\/]react-query/ },
          ],
        },
      },
    },
  },
}))
