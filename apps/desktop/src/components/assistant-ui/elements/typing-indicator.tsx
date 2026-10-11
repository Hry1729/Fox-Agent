import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'
import './fox-elements.css'
import { TypingIndicator as UpstreamTyping } from '../upstream/elements/typing-indicator'

// Adapted from assistant-ui elements-typing-indicator. The bare form fits the
// scroll button; its host owns the accessible name and the running condition.
export function TypingIndicator({ variant = 'bubble', className, ...props }: ComponentProps<'span'> & { variant?: 'bubble' | 'bare' }) {
  return <span aria-hidden="true" className="aui-typing-wrapper" {...props}><UpstreamTyping variant={variant} className={cn('aui-typing-indicator', className)} /></span>
}
