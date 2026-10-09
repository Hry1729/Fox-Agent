import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import path from 'node:path'

/**
 * The row's fade is a layout promise the DOM tests cannot see: happy-dom has no
 * layout and the stylesheets are not mounted. This guard reads the real stylesheet
 * and holds it to exactly one claim — the ellipsis and the right-edge fade belong
 * to the trailing source/summary region alone, never to the row, its trigger or the
 * action name. The DOM tests next door assert what the row actually renders.
 */
const css = readFileSync(path.join(import.meta.dir, '..', 'src', 'styles', 'workbench.css'), 'utf8')

/** Every declaration block written for one selector, in source order. */
function blocks(selector: string) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  return [...css.matchAll(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g'))].map((match) => match[1])
}

const declares = (selector: string, property: string) =>
  blocks(selector).some((body) => new RegExp(`(^|;|\\s)${property}\\s*:`, 'm').test(body.replace(/\/\*[\s\S]*?\*\//g, '')))

/** The bodies of every rule whose *selector list* contains this exact selector, so
 *  a rule shared by two regions (which is how they are guaranteed to fade alike) is
 *  read for both of them. Comments are stripped first: they sit between rules and
 *  would otherwise be glued to the next rule's selector list. */
function sharedBlocks(selector: string) {
  return [...css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter((match) => match[1].split(',').map((part) => part.trim()).includes(selector))
    .map((match) => match[2])
}

describe('process row slot styling', () => {
  test('the mask and the ellipsis live on the source/summary region only', () => {
    expect(declares('.fox-run-step-tail', 'mask-image')).toBe(true)
    expect(declares('.fox-run-step-tail', '-webkit-mask-image')).toBe(true)
    expect(declares('.fox-run-step-tail', 'text-overflow')).toBe(true)
    // Nothing else in the slot line is masked or clipped.
    for (const selector of ['.fox-run-step-action', '.fox-run-step-separator', '.fox-run-step-path-slot']) {
      expect({ selector, mask: declares(selector, 'mask-image') }).toEqual({ selector, mask: false })
      expect({ selector, mask: declares(selector, '-webkit-mask-image') }).toEqual({ selector, mask: false })
    }
    // The action name is never clipped: it keeps its full text whatever the width.
    expect(declares('.fox-run-step-action', 'overflow')).toBe(false)
    expect(declares('.fox-run-step-action', 'text-overflow')).toBe(false)
  })

  test('the whole row, its trigger and its icon are never masked', () => {
    for (const selector of ['.fox-runtime-step-row', '.fox-runtime-step-trigger', '.fox-runtime-step-body', '.fox-runtime-step-icon', '.fox-run-status-text']) {
      expect({ selector, mask: declares(selector, 'mask-image') }).toEqual({ selector, mask: false })
    }
    // The label owns the fade for plain-text rows (a settled thought's summary) and
    // turns it off when that label is built from slots.
    expect(declares('.fox-run-status-label', 'mask-image')).toBe(true)
    expect(blocks('.fox-run-status-label.has-slots').join(' ')).toMatch(/mask-image:\s*none/)
    expect(blocks('.fox-run-status-label.has-slots').join(' ')).toMatch(/-webkit-mask-image:\s*none/)
  })

  /**
   * The fade is a *measured* state, not a fixed decoration: every one-line region
   * declares `none` and only gains the gradient under the class that
   * `features/chat/row-overflow.ts` toggles from real layout. That is what keeps a
   * short title fully painted. The DOM tests next door drive the measurement and the
   * class toggle; this guard holds the stylesheet to the shape that makes it work.
   */
  test('the fade is conditional on a measured overflow, and stops short of the ellipsis', () => {
    const FADE = 'linear-gradient(to right, #000 0, #000 calc(100% - 2.2em), transparent calc(100% - 1.2em), #000 calc(100% - 1em), #000 100%)'
    /** Every `mask-image` value written for one selector, in source order. The
     *  leading boundary keeps `-webkit-mask-image` from counting as `mask-image`. */
    const masks = (selector: string) => sharedBlocks(selector)
      .flatMap((body) => [...body.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/(?:^|[;\s])mask-image:\s*([^;]+);/g)])
      .map((match) => match[1].trim())
    for (const selector of ['.fox-run-status-label', '.fox-run-step-tail']) {
      // No region is faded by default: a short title stays fully painted.
      expect({ selector, base: masks(selector) }).toEqual({ selector, base: ['none'] })
      // It fades only once a real measurement toggles the class, and the fade is a
      // short trailing strip rather than a mask over the whole line.
      expect({ selector, overflow: masks(`${selector}.is-overflowing`) }).toEqual({ selector, overflow: [FADE] })
      // Both the standard and the prefixed property carry it.
      expect({ selector, prefixed: sharedBlocks(`${selector}.is-overflowing`).some((body) => /(^|[;\s])-webkit-mask-image:\s*linear-gradient/.test(body)) })
        .toEqual({ selector, prefixed: true })
    }
    // The gradient returns to opaque over the last em and never reaches
    // `transparent 100%`: the ellipsis the browser paints at the right edge has to
    // stay readable, and the leading text must not be dimmed.
    expect(FADE).not.toMatch(/transparent 100%/)
    expect(FADE).toMatch(/#000 0, #000 calc\(100% - 2\.2em\)/)
  })
})
