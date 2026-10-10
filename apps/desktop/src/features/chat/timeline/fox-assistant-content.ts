import { normalizeAssistantMarkdown } from '@/features/conversations/model/assistant-presentation'

/**
 * Splits an assistant message's raw content into the visible answer and the
 * `<think>`-style reasoning it carries inline. Pure text logic, kept out of the
 * component files so the thread converter (and its tests) can use it without
 * pulling the timeline's component tree into the import graph.
 */
export function splitAssistantContent(content: string) {
  const reasoning: string[] = []
  const answer = normalizeAssistantMarkdown(content.replace(/<((?:mm:)?(?:think|thinking|analysis))>([\s\S]*?)<\/\1>|<(?:(?:mm:)?(?:think|thinking|analysis))>([\s\S]*)$|^[\s\S]*?<\/(?:(?:mm:)?(?:think|thinking|analysis))>\s*/gi, (_match, _tag: string | undefined, closed: string | undefined, open: string | undefined) => {
    const value = (closed ?? open ?? '').trim()
    if (value) reasoning.push(value)
    return ''
  }).replace(/<\/(?:(?:mm:)?(?:think|thinking|analysis))>/gi, '').trim())
  return { answer, reasoning: normalizeAssistantMarkdown(reasoning.join('\n\n')) }
}
