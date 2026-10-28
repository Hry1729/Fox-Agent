import { useEffect, useRef } from 'react'
import type { KnowledgeSourceLocator } from '@/features/workspace/types'

export function localSourceRange(text: string, locator: KnowledgeSourceLocator) {
  const excerpt = locator.excerpt?.trim()
  const bytes = /^bytes:(\d+)-(\d+)$/.exec(locator.anchor ?? '')
  if (bytes) {
    const encoded = new TextEncoder().encode(text)
    const start = Number(bytes[1]), end = Number(bytes[2])
    if (start < end && end <= encoded.length) {
      const decoder = new TextDecoder('utf-8', { fatal: true })
      try {
        const before = decoder.decode(encoded.slice(0, start))
        const match = decoder.decode(encoded.slice(start, end))
        if (!excerpt || match.trimStart().startsWith(excerpt)) return { start: before.length, end: before.length + match.length }
      } catch { /* A stale offset can split a UTF-8 character; try the excerpt. */ }
    }
  }
  if (excerpt) {
    const start = text.indexOf(excerpt)
    if (start >= 0) return { start, end: start + excerpt.length }
  }
  return null
}

export function LocalKnowledgeSourceText({ text, locator }: { text: string; locator: KnowledgeSourceLocator }) {
  const range = localSourceRange(text, locator)
  const highlight = useRef<HTMLElement | null>(null)
  useEffect(() => { highlight.current?.scrollIntoView?.({ block: 'center' }) }, [range?.start, range?.end, text])
  return <pre className="fox-local-files-text-preview">{range
    ? <>{text.slice(0, range.start)}<mark ref={highlight} aria-label="引用片段">{text.slice(range.start, range.end)}</mark>{text.slice(range.end)}</>
    : <><span role="status">未能在当前预览中定位引用片段。</span>{'\n\n'}{text}</>}</pre>
}
