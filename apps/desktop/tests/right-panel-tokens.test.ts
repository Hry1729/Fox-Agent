import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'

const panel = readFileSync(new URL('../src/features/chat/right-panel/right-panel.css', import.meta.url), 'utf8')
const sections = readFileSync(new URL('../src/components/fox-ui/side-section.css', import.meta.url), 'utf8')

test('平铺侧栏只使用 Fox 和上下文分类 token，字号随应用缩放', () => {
  for (const css of [panel, sections]) {
    expect(css).not.toContain('--rp-')
    expect(css).not.toMatch(/var\(--(?!fox-|ctx-)/)
    expect(css).not.toMatch(/font-size:\s*\d/)
    for (const [, radius] of css.matchAll(/border-radius:\s*([^;]+);/g)) {
      expect(radius === '0' || /^var\(--fox-radius-[\w-]+\)$/.test(radius)).toBe(true)
    }
    for (const [, selector, declarations] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      if (selector.includes('.fox-summary-drag-overlay')) continue
      for (const [, shadow] of declarations.matchAll(/box-shadow:\s*([^;]+);/g)) expect(shadow.trim()).toBe('none')
    }
  }
})
