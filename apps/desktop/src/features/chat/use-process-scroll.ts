import { useCallback, useLayoutEffect, useMemo, useRef, useState, type DOMAttributes, type KeyboardEvent, type RefObject } from 'react'
import { ProcessScrollFollow, processScrollMetrics } from './process-scroll-follow'

type ScrollEdges = { up: boolean; down: boolean }
const REST: ScrollEdges = { up: false, down: false }
const SCROLL_KEYS = new Set(['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '])

/** One process group's local scrolling, independent of the conversation viewport. */
export function useProcessScroll(
  bodyRef: RefObject<HTMLDivElement | null>,
  contentRef: RefObject<HTMLDivElement | null>,
  open: boolean,
  capped: boolean,
  contentVersion: unknown,
): {
  edges: ScrollEdges
  events: Pick<DOMAttributes<HTMLDivElement>, 'onScroll' | 'onWheel' | 'onTouchStart' | 'onPointerDown' | 'onKeyDown'>
  initialize: (position: 'top' | 'bottom') => void
} {
  const [follow] = useState(() => new ProcessScrollFollow())
  const [edges, setEdges] = useState<ScrollEdges>(REST)
  const pendingOpen = useRef<'top' | 'bottom' | null>(null)
  const previous = useRef({ open: false, capped: false })
  const savedTop = useRef<number | null>(null)
  const lastCappedTop = useRef<number | null>(null)
  const lastBody = useRef<HTMLDivElement | null>(null)
  const initialize = useCallback((position: 'top' | 'bottom') => { pendingOpen.current = position }, [])
  const updateEdges = useCallback(() => {
    const body = bodyRef.current
    const metrics = body && open && capped ? processScrollMetrics(body) : null
    if (body) lastBody.current = body
    if (metrics) lastCappedTop.current = metrics.top
    const next = metrics ? { up: metrics.top > 2, down: metrics.bottom - metrics.top > 2 } : REST
    setEdges(current => current.up === next.up && current.down === next.down ? current : next)
  }, [bodyRef, capped, open])
  const sync = useCallback((cause: 'growth' | 'scroll' | 'scrollend') => {
    const body = bodyRef.current
    if (!body || !open || !capped) return
    if (cause === 'scroll') follow.sample(processScrollMetrics(body))
    else if (cause === 'scrollend') follow.settle(processScrollMetrics(body))
    if (cause !== 'scroll') follow.followGrowth(body)
    updateEdges()
  }, [bodyRef, capped, follow, open, updateEdges])
  const interrupt = useCallback(() => {
    const body = bodyRef.current
    if (body && open && capped) follow.interrupt(body)
  }, [bodyRef, capped, follow, open])
  const events = useMemo(() => ({
    onScroll: (event: { target: EventTarget; currentTarget: EventTarget }) => {
      if (event.target === event.currentTarget) sync('scroll')
    },
    onWheel: interrupt,
    onTouchStart: interrupt,
    onPointerDown: interrupt,
    onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
      if (!event.defaultPrevented && SCROLL_KEYS.has(event.key)) interrupt()
    },
  }), [interrupt, sync])

  useLayoutEffect(() => {
    const body = bodyRef.current
    const before = previous.current
    if (body && before.open && before.capped && open && !capped) {
      // The uncapped CSS may already have reset scrollTop by this layout effect.
      savedTop.current = lastCappedTop.current ?? body.scrollTop
      follow.interrupt(body)
    }
    if (!open) {
      if (body) follow.interrupt(body)
      follow.reset()
      savedTop.current = null
      lastCappedTop.current = null
      pendingOpen.current = null
    } else if (body && capped && (!before.open || !before.capped)) {
      const initial = pendingOpen.current
      if (initial) follow.initialize(body, initial)
      else if (savedTop.current !== null) follow.restore(body, savedTop.current)
      else follow.reset()
      pendingOpen.current = null
      savedTop.current = null
    }
    previous.current = { open, capped }
    updateEdges()
  }, [bodyRef, capped, follow, open, updateEdges])

  useLayoutEffect(() => { sync('growth') }, [contentVersion, sync])

  useLayoutEffect(() => {
    const body = bodyRef.current
    if (!body || !open || !capped || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(() => { sync('growth') })
    const onScrollEnd = (event: Event) => { if (event.target === body) sync('scrollend') }
    body.addEventListener('scrollend', onScrollEnd)
    observer.observe(body)
    if (contentRef.current) observer.observe(contentRef.current)
    return () => {
      follow.interrupt(body)
      observer.disconnect()
      body.removeEventListener('scrollend', onScrollEnd)
    }
  }, [bodyRef, capped, contentRef, follow, open, sync])

  useLayoutEffect(() => () => {
    if (lastBody.current) follow.interrupt(lastBody.current)
  }, [bodyRef, follow])

  return { edges, events, initialize }
}
