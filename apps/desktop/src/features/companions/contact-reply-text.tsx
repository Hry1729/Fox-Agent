import { memo, useState } from 'react'
import { parseMarkdownIntoBlocks } from 'streamdown'
import { MarkdownResponse } from '@/features/chat/fox-shared'

const headingOrDefinition = /^ {0,3}(?:#{1,6}\s|\[[^\]\n]+\]:)/mu
const structuredBlock = /^ {0,3}(?:#{1,6}\s|`{3,}|~{3,}|\||>|[-*+]\s|\d+[.)]\s)|^ {4}|^\t/u

export default memo(function ContactReplyText({ text, hasReport }: { text: string; hasReport: boolean }) {
  const [expanded, setExpanded] = useState(false)
  const body = text.trimEnd()
  const lines = body.split(/\r?\n/u)
  const long = body.length > 1000 || lines.length > 16

  if (long) {
    const preview = body.length > 1000 ? body.slice(0, 600) : lines.slice(0, 16).join('\n')
    const paragraphEnd = preview.lastIndexOf('\n\n')
    const shortened = paragraphEnd >= 160 ? preview.slice(0, paragraphEnd) : preview.trimEnd()
    const summary = hasReport
      ? parseMarkdownIntoBlocks(preview).find(block => block.trim() && !structuredBlock.test(block))?.trim()
      : undefined
    const excerpt = summary && summary.length > 240 ? `${summary.slice(0, 240).trimEnd()}…` : summary
    return <div className="fox-contact-bubble">
      <MarkdownResponse className="fox-answer-response">{hasReport
        ? excerpt || '完整回复见附件。'
        : expanded ? text : `${shortened}…`}</MarkdownResponse>
      {!hasReport && <button type="button" className="fox-contact-reply-disclosure" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>
        {expanded ? '收起全文' : '展开全文'}
      </button>}
    </div>
  }

  // Headings and reference definitions can belong to another Markdown block.
  const bubbles = headingOrDefinition.test(text) ? [text] : parseMarkdownIntoBlocks(text).filter(block => block.trim())
  return bubbles.map((block, index) => <div className="fox-contact-bubble" key={index}>
    <MarkdownResponse className="fox-answer-response">{block}</MarkdownResponse>
  </div>)
})
