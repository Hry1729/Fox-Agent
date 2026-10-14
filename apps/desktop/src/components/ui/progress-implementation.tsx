'use client'

import type * as React from 'react'
import { isValidElement } from 'react'
import type { Progress as RadixProgress } from 'radix-ui'
import { Progress as AnimateProgress, ProgressTrack } from '@/components/animate-ui/components/base/progress'
import { cn } from '@/lib/utils'

type ProgressProps = React.ComponentProps<typeof RadixProgress.Root>

/** Existing single-element API, backed by Animate UI's Base Progress and animated indicator. */
function Progress({ className, value = 0, max = 100, getValueLabel, asChild, children, ...props }: ProgressProps) {
  const safeMax = Number.isFinite(max) && max > 0 ? max : 100
  const safeValue = value == null || !Number.isFinite(value) ? null : Math.min(safeMax, Math.max(0, value))
  const valueLabel = safeValue == null ? undefined : getValueLabel?.(safeValue, safeMax)
  return <AnimateProgress {...props} render={asChild && isValidElement<Record<string, unknown>>(children) ? children : undefined} value={safeValue} max={safeMax} aria-valuetext={props['aria-valuetext'] ?? valueLabel} className={cn('relative h-1 w-full overflow-hidden rounded-md bg-muted', className)}>
    <ProgressTrack className="absolute inset-0 h-full rounded-[inherit] bg-transparent" />
    {!asChild && children}
  </AnimateProgress>
}

export { Progress, type ProgressProps }
