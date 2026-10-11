import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'
import './fox-elements.css'
import { ThinkingIndicator as UpstreamThinking } from '../upstream/elements/thinking-indicator'

// The upstream dot + keyed shimmer label is rendered inline so it is valid
// inside Fox's whole-row disclosure button. Colors come from the active theme.
export function ThinkingIndicator({ label, active = true, showDot = active, className, ...props }: ComponentProps<'span'> & { label: string; active?: boolean; showDot?: boolean }) {
  return <UpstreamThinking label={label} active={active} showDot={showDot} className={cn('aui-thinking-indicator', className)} {...props} />
}
