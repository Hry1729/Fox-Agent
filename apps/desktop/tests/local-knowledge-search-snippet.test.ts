import { describe, expect, test } from 'bun:test'
import { searchSnippetParts } from '../src/features/local-knowledge/search-snippet'

describe('local knowledge search snippets', () => {
  test('centers context around the first matching word', () => {
    const result = searchSnippetParts('开头'.repeat(40) + '资料库' + '后文'.repeat(40), '资料库', 12)
    expect(result.match).toBe('资料库')
    expect(Array.from(result.before)).toHaveLength(12)
    expect(Array.from(result.after)).toHaveLength(12)
    expect(result.prefixEllipsis).toBe(true)
    expect(result.suffixEllipsis).toBe(true)
  })
  test('uses the earliest keyword and matches case without altering source text', () => {
    expect(searchSnippetParts('Use FOX to read documents.', 'documents fox').match).toBe('FOX')
  })
  test('keeps Unicode characters whole at snippet boundaries', () => {
    const result = searchSnippetParts('😀😀😀目标🐱🐱🐱', '目标', 2)
    expect(result.before).toBe('😀😀')
    expect(result.after).toBe('🐱🐱')
  })
  test('normalizes line breaks and keeps markup as plain source text', () => {
    const result = searchSnippetParts('<img src=x>\nFox\n<script>text</script>', 'Fox')
    expect(result.before).toBe('<img src=x> ')
    expect(result.match).toBe('Fox')
    expect(result.after).toBe(' <script>text</script>')
  })
  test('returns a bounded line for empty queries or no match', () => {
    expect(searchSnippetParts('', '')).toEqual({ before: '', match: '', after: '', prefixEllipsis: false, suffixEllipsis: false })
    expect(searchSnippetParts('abcdefghi', 'missing', 2)).toEqual({ before: 'abcd', match: '', after: '', prefixEllipsis: false, suffixEllipsis: true })
  })
})
