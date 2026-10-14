import { isValidElement, cloneElement } from 'react'
import type { ProgressProps } from './progress-implementation'
import { cn } from '@/lib/utils'
import { deferredModule } from '@/components/fox-ui/use-deferred-module'
const implementation = deferredModule(() => import('./progress-implementation'))
function Progress({ className, value = 0, max = 100, getValueLabel, asChild, children, ...props }: ProgressProps) {
  const loaded = implementation.useModule()
  if (loaded) return <loaded.Progress {...{ className, value, max, getValueLabel, asChild, children, ...props }} />
  const safeMax = Number.isFinite(max) && max > 0 ? max : 100
  const safeValue = value == null || !Number.isFinite(value) ? null : Math.min(safeMax, Math.max(0, value))
  const attributes = { ...props, role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': safeMax, 'aria-valuenow': safeValue ?? undefined, 'aria-valuetext': props['aria-valuetext'] ?? (safeValue == null ? undefined : getValueLabel?.(safeValue, safeMax)), 'data-state': safeValue == null ? 'indeterminate' : safeValue === safeMax ? 'complete' : 'loading', className: cn('relative h-1 w-full overflow-hidden rounded-md bg-muted', className) }
  if (asChild && isValidElement<Record<string, unknown>>(children)) return cloneElement(children, attributes)
  return <div {...attributes}><div className="absolute inset-0 h-full rounded-[inherit] bg-transparent"><div className="bg-primary h-full w-full rounded-full" style={{ transform: `translateX(-${100 - (safeValue ?? 0) / safeMax * 100}%)` }} /></div>{children}</div>
}
export { Progress, type ProgressProps }
