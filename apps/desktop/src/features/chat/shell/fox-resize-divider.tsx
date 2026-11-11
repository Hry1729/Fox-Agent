import { useRef } from 'react'

export interface ResizeBounds { width: number; min: number; max: number }

export function ResizeDivider({ className, label, width, direction, measure, onResize }: {
  className: string
  label: string
  width: number
  direction: 1 | -1
  measure: () => ResizeBounds
  onResize: (width: number) => void
}) {
  const drag = useRef<(ResizeBounds & { x: number }) | null>(null)
  const apply = (bounds: ResizeBounds, delta: number) => onResize(Math.min(bounds.max, Math.max(bounds.min, bounds.width + delta)))
  return <div className={className} role="separator" tabIndex={0} aria-label={label} aria-orientation="vertical" aria-valuemin={direction === 1 ? 220 : 280} aria-valuemax={direction === 1 ? 456 : 760} aria-valuenow={Math.round(width)}
    onPointerDown={(event) => {
      if (event.button !== 0) return
      event.preventDefault()
      drag.current = { ...measure(), x: event.clientX }
      event.currentTarget.dataset.resizing = 'true'
      event.currentTarget.setPointerCapture(event.pointerId)
    }}
    onPointerMove={(event) => { if (drag.current) apply(drag.current, direction * (event.clientX - drag.current.x)) }}
    onPointerUp={(event) => { drag.current = null; delete event.currentTarget.dataset.resizing }}
    onPointerCancel={(event) => { drag.current = null; delete event.currentTarget.dataset.resizing }}
    onLostPointerCapture={(event) => { drag.current = null; delete event.currentTarget.dataset.resizing }}
    onKeyDown={(event) => {
      const bounds = measure()
      if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
        event.preventDefault()
        apply(bounds, direction * (event.key === 'ArrowLeft' ? -1 : 1) * (event.shiftKey ? 24 : 8))
      } else if (event.key === 'Home' || event.key === 'End') {
        event.preventDefault()
        onResize(event.key === 'Home' ? bounds.min : bounds.max)
      }
    }} />
}
