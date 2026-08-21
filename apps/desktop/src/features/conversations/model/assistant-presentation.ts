const MARKDOWN_CODE_PATTERN = /(```[\s\S]*?(?:```|$)|~~~[\s\S]*?(?:~~~|$)|`[^`\n]*`)/g
const EMOJI_PATTERN = /(?:[\u{1F1E6}-\u{1F1FF}]{2}|[#*0-9]\uFE0F?\u20E3|\p{Extended_Pictographic}(?:[\uFE0E\uFE0F])?(?:\p{Emoji_Modifier})?(?:\u200D\p{Extended_Pictographic}(?:[\uFE0E\uFE0F])?(?:\p{Emoji_Modifier})?)*)/gu

function normalizeProse(segment: string) {
  return segment.split('\n').map((line) => {
    let normalized = line.replace(EMOJI_PATTERN, '')
    const removedEmoji = normalized !== line
    const leading = normalized.match(/^[\t \u00A0\u3000]+/)?.[0] ?? ''
    const content = normalized.slice(leading.length)
    const semanticIndent = /^(?:[-+*]|\d+[.)]|>|#{1,6})[ \t]/.test(content)
    if (leading && !semanticIndent) normalized = content
    if (removedEmoji) {
      normalized = normalized
        .replace(/(?<=\S)[ \t]{2,}(?=\S)/g, ' ')
        .replace(/[ \t]+$/, '')
    }
    return normalized
  }).join('\n')
}

export function normalizeAssistantMarkdown(content: string) {
  return content
    .split(MARKDOWN_CODE_PATTERN)
    .map((segment, index) => index % 2 === 1 ? segment : normalizeProse(segment))
    .join('')
    .trim()
}
