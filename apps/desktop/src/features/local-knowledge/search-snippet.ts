export interface SearchSnippetParts {
  before: string
  match: string
  after: string
  prefixEllipsis: boolean
  suffixEllipsis: boolean
}

export function searchSnippetParts(content: string, query: string, contextLength = 48): SearchSnippetParts {
  const text = content.replace(/\s+/gu, ' ').trim()
  const lower = text.toLocaleLowerCase()
  const term = query.trim()
  const tokens = new Set([term, ...Array.from(new Intl.Segmenter('zh', { granularity: 'word' }).segment(term))
    .filter((part) => part.isWordLike).map((part) => part.segment)])
  let first = -1
  let length = 0
  if (lower.length === text.length) {
    for (const token of tokens) {
      if (!token) continue
      const needle = token.toLocaleLowerCase()
      if (needle.length !== token.length) continue
      const index = lower.indexOf(needle)
      if (index >= 0 && (first < 0 || index < first || (index === first && token.length > length))) {
        first = index
        length = token.length
      }
    }
  }
  if (first < 0) {
    const points = Array.from(text)
    return { before: points.slice(0, contextLength * 2).join(''), match: '', after: '', prefixEllipsis: false, suffixEllipsis: points.length > contextLength * 2 }
  }
  const before = Array.from(text.slice(0, first))
  const after = Array.from(text.slice(first + length))
  return {
    before: before.slice(-contextLength).join(''),
    match: text.slice(first, first + length),
    after: after.slice(0, contextLength).join(''),
    prefixEllipsis: before.length > contextLength,
    suffixEllipsis: after.length > contextLength,
  }
}
