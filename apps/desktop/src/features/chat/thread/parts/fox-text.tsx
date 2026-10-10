import { MarkdownResponse } from '../../fox-shared'
import type { TextMessagePartProps } from '@assistant-ui/react'

/**
 * The `text` part renderer: the existing Streamdown response (CJK, math,
 * mermaid, local-fullscreen tables, collapsible code blocks) — the same renderer
 * the legacy timeline used, lazily loaded through `MarkdownResponse`.
 */
export function FoxTextPart(part: TextMessagePartProps) {
  if (!part.text) return null
  return <MarkdownResponse className="fox-thread-answer">{part.text}</MarkdownResponse>
}
