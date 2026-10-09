import { OVERFLOW_CLASS, useOverflowFade } from '../row-overflow'
import './fox-row-shimmer.css'

export const FOX_ASSISTANT_AVATAR = '/avatars/fox-assistant.png'

export function FoxAssistantAvatar({ size = 'sm' }: { size?: 'sm' | 'md' | 'lg' }) {
  return <img className={`fox-assistant-avatar is-${size}`} src={FOX_ASSISTANT_AVATAR} alt="" aria-hidden="true" width={28} height={28} />
}

/** One readable label and one inert, presentation-only copy share the row sweep.
 *  The label clips its own single line, so it — and only it — earns the trailing
 *  fade when a real measurement says the line is cut (row-overflow.ts). */
export function RunStatusText({ text, active = true }: { text: string; active?: boolean }) {
  const { ref, overflowing } = useOverflowFade<HTMLSpanElement>(text)
  return <span className="fox-run-status-text fox-row-shimmer" data-active={active}>
    <span ref={ref} className={`fox-run-status-label${overflowing ? ` ${OVERFLOW_CLASS}` : ''}`}>{text}</span>
    {active && <span className="fox-row-shimmer-decoration" aria-hidden="true" inert>
      <span className="fox-row-shimmer-sweep">
        <span className="fox-row-shimmer-highlight fox-row-shimmer-status-copy" data-shimmer-text={text} />
      </span>
    </span>}
  </span>
}
