import { i18n } from '@lingui/core'
// Registers the DOM matchers (`toBeInTheDocument`, `toHaveAttribute`,
// `toHaveStyle`) on Vitest's `expect`. Imported for the side effect: without
// it those matchers do not exist, and using one fails with "Invalid Chai
// property" rather than with the assertion it was meant to make.
import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterEach, beforeAll } from 'vitest'

/**
 * Test environment setup.
 *
 * The catalog is activated with the source messages rather than a compiled
 * one: a component test asserts on the English a developer wrote, so it does
 * not break every time a translation is added — and it does not need a build
 * step to run.
 */
beforeAll(() => {
  i18n.loadAndActivate({ locale: 'en', messages: {} })

  // jsdom does not implement scrolling and logs "Not implemented" for every
  // router navigation. Stubbed rather than filtered from the output, because
  // a real error in that stream should still stand out.
  window.scrollTo = () => {}
})

// Without this, a component from one test is still in the document during the
// next, and a query for "the heading" matches two — which reads as a
// component bug rather than a leaked render.
afterEach(cleanup)
