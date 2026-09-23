import { i18n } from '@lingui/core'

/**
 * Locale activation (ADR-0011).
 *
 * Catalogs are compiled at build time and imported dynamically, so only the
 * active locale's messages are downloaded. `failOnMissing` in the Vite config
 * makes an untranslated string a build failure rather than a runtime fallback,
 * which is the whole point of compile-time extraction.
 */

export const LOCALES = ['en', 'es'] as const
export type Locale = (typeof LOCALES)[number]

export const DEFAULT_LOCALE: Locale = 'en'

/**
 * The best supported locale for a browser's preferences.
 *
 * Matches on the language subtag, so `es-419` and `es-MX` both reach `es`
 * rather than falling back to English — a reader in Mexico getting English
 * because the region did not match exactly is the failure this avoids.
 */
export function negotiate(preferences: readonly string[]): Locale {
  for (const preference of preferences) {
    const language = preference.toLowerCase().split('-')[0]
    const match = LOCALES.find((locale) => locale === language)
    if (match) return match
  }
  return DEFAULT_LOCALE
}

export async function activate(locale: Locale): Promise<void> {
  const { messages } = await import(`./catalogs/${locale}/messages.po`)
  i18n.loadAndActivate({ locale, messages })
}

export { i18n }
