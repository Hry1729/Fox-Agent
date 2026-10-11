import { TypingIndicator } from '@/components/assistant-ui/elements/typing-indicator'

/** 三颗点的思考态。按钮里用它替掉箭头，所以本身不进可读名——
 * 宿主控件的 aria-label 才是那句"定位到最新消息"。 */
export function FoxThinkingDots({ className }: { className?: string }) {
  return <TypingIndicator variant="bare" className={className} />
}
