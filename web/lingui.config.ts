import { defineConfig } from '@lingui/cli'
import { formatter } from '@lingui/format-po'

// ADR-0011: source text is the message ID, so there is no key namespace to
// keep in step with copy, and a translator sees the actual English.
export default defineConfig({
  sourceLocale: 'en',
  // Spanish ships from the first release so extraction, ICU plural handling,
  // and the design system's ~30% string-expansion tolerance are exercised
  // rather than assumed.
  locales: ['en', 'es'],
  catalogs: [
    {
      path: '<rootDir>/src/shared/i18n/catalogs/{locale}/messages',
      include: ['<rootDir>/src'],
    },
  ],
  // Lingui 6 no longer accepts a string format name; formatters are separate
  // packages. lineNumbers off keeps catalog diffs to actual message changes
  // rather than churning on every edit above them.
  format: formatter({ lineNumbers: false }),
})
