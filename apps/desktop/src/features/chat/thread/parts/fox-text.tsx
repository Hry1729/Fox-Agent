import { StreamingText } from '../../components/StreamingText'
import { useAuiState } from '@assistant-ui/react'
import type { TextMessagePartProps } from '@assistant-ui/react'

/**
 * The `text` part renderer: the existing Streamdown response (CJK, math,
 * mermaid, local-fullscreen tables, collapsible code blocks) — the same renderer
 * the legacy timeline used, lazily loaded through `MarkdownResponse`.
 */
export function FoxTextPart(part: TextMessagePartProps) {
  const streaming = useAuiState(state => state.message.status?.type === 'running')
  if (!part.text) return null
  return <StreamingText className="fox-thread-answer" text={part.text} streaming={streaming} />
}
