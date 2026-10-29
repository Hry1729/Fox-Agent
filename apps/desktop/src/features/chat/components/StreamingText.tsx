import { MarkdownResponse } from '../fox-shared'
import '@/components/assistant-ui/elements/fox-elements.css'
import { StreamingCursor } from '@/components/assistant-ui/upstream/elements/streaming-text'

export function StreamingText({ text, streaming, className }: { text: string; streaming: boolean; className?: string }) {
  return <div data-slot="streaming-text" data-streaming={streaming}><MarkdownResponse className={className} isAnimating={streaming} animated={false}>{text}</MarkdownResponse>{streaming && text && <StreamingCursor />}</div>
}
