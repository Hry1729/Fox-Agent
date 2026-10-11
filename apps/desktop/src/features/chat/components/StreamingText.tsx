import { MarkdownResponse } from '../fox-shared'
import '@/components/assistant-ui/elements/fox-elements.css'
import { StreamingCursor } from '@/components/assistant-ui/upstream/elements/streaming-text'

// Streamdown marks only new Markdown text leaves. The registry component renders
// those leaves with its fresh-text spans, without flattening the Markdown tree.
const animation = { animation: 'fadeIn', duration: 500, easing: 'ease-out', sep: 'char' as const, stagger: 0 }

export function StreamingText({ text, streaming, className }: { text: string; streaming: boolean; className?: string }) {
  return <div data-slot="streaming-text" data-streaming={streaming}><MarkdownResponse className={className} isAnimating={streaming} animated={streaming ? animation : false}>{text}</MarkdownResponse>{streaming && text && <StreamingCursor />}</div>
}
