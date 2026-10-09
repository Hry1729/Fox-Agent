import { useCallback, useEffect, useLayoutEffect, useState } from 'react'

/** One element's own layout facts, exactly as the browser reports them. */
export interface LineMetrics {
  scrollWidth: number
  clientWidth: number
}

/** The single class a caller toggles once a real measurement says the line is cut. */
export const OVERFLOW_CLASS = 'is-overflowing'

/**
 * Does this one-line box really cut its own content off?
 *
 * The question cannot be answered from the text. Twelve CJK characters, twenty-four
 * Latin ones and one long path all occupy different widths, and the width available
 * to a row changes with the sidebar, the zoom level and the font — so only layout
 * knows, and only layout decides. One pixel of tolerance absorbs sub-pixel rounding
 * (a box a fraction of a pixel wider than its text is not truncated), and a box with
 * no measurable width — a hidden row, or a test environment without layout, where
 * both numbers are 0 — is never treated as overflowing. A short title therefore
 * keeps its full, unmasked paint.
 */
export function isOverflowing({ scrollWidth, clientWidth }: LineMetrics) {
  if (!(clientWidth > 0)) return false
  return scrollWidth - clientWidth > 1
}

/**
 * Measures one element that clips its own single line and reports whether the line
 * is cut, so the caller can add `OVERFLOW_CLASS` to that element alone and nowhere
 * else.
 *
 * Two triggers, because neither alone sees everything:
 *
 *  - the rendered content, so text that grows while streaming is re-read even when
 *    the box itself is already pinned to the line width (a clip box does not resize
 *    when the text inside it grows, so a ResizeObserver alone would never fire);
 *  - one `ResizeObserver` per measured element, which is the only thing that can see
 *    what no re-render sees: a narrower or wider sidebar, a resized or zoomed window,
 *    a font that changes metrics, and a row that is hidden and shown again.
 *
 * The observer is created for that element and disconnected when it detaches, so a
 * long process list never accumulates observers and nothing is polled per frame.
 */
export function useOverflowFade<T extends HTMLElement>(content: unknown, enabled = true) {
  const [element, setElement] = useState<T | null>(null)
  const [overflowing, setOverflowing] = useState(false)
  const ref = useCallback((node: T | null) => { setElement(node) }, [])
  const read = useCallback(() => {
    const next = enabled && element ? isOverflowing(element) : false
    setOverflowing((previous) => (previous === next ? previous : next))
  }, [element, enabled])
  useLayoutEffect(() => { read() }, [read, content])
  useEffect(() => {
    if (!enabled || !element || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(() => { read() })
    observer.observe(element)
    return () => observer.disconnect()
  }, [element, enabled, read])
  return { ref, overflowing }
}
