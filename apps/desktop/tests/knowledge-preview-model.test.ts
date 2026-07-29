import { describe, expect, test } from 'bun:test'
import {
  MAX_TABLE_COLUMNS,
  MAX_TABLE_ROWS,
  MAX_PDF_ZOOM,
  MIN_PDF_ZOOM,
  TABLE_ROW_HEIGHT,
  byteRanges,
  codeLanguage,
  formatKnowledgeDocumentInfo,
  formatJsonPreview,
  parseDelimitedPreview,
  parsePresentationSlides,
  movePdfPage,
  normalizePdfViewport,
  previewKind,
  previewSourceStrategy,
  virtualRowWindow,
  zoomPdf,
} from '../src/features/knowledge/knowledge-preview-model'
import { assertSafeZipArchive } from '../src/features/knowledge/knowledge-archive-safety'
import { normalizeKnowledgeSourceLocator } from '../src/features/knowledge/knowledge-source-locator'

describe('knowledge preview model', () => {
  test('selects previewers case-insensitively', () => {
    expect(previewKind('manual.PDF')).toBe('pdf')
    expect(previewKind('photo.JPEG')).toBe('image')
    expect(previewKind('readme.MD')).toBe('markdown')
    expect(previewKind('data.TSV')).toBe('csv')
    expect(previewKind('proposal.DOCX')).toBe('docx')
    expect(previewKind('budget.XLSX')).toBe('spreadsheet')
    expect(previewKind('slides.PPTX')).toBe('presentation')
    expect(previewKind('legacy.DOC')).toBe('legacy-office')
    expect(previewKind('archive.zip')).toBe('unsupported')
  })

  test('uses complete managed cache files for binary document previewers', () => {
    expect(previewSourceStrategy('pdf')).toBe('cache')
    expect(previewSourceStrategy('docx')).toBe('cache')
    expect(previewSourceStrategy('spreadsheet')).toBe('cache')
    expect(previewSourceStrategy('presentation')).toBe('cache')
    expect(previewSourceStrategy('image')).toBe('cache')
    expect(previewSourceStrategy('text')).toBe('direct')
  })

  test('formats complete document information for the file actions menu', () => {
    expect(formatKnowledgeDocumentInfo({
      knowledgeBase: '设备资料',
      filename: '轨道吊说明书.docx',
      kind: 'Word 文档',
      size: '2.5 MB',
      updatedAt: '2026/7/25',
      status: 'completed',
      documentId: 'document-1',
    })).toBe([
      '知识库：设备资料',
      '文件名：轨道吊说明书.docx',
      '类型：Word 文档',
      '大小：2.5 MB',
      '更新时间：2026/7/25',
      '处理状态：completed',
      '文档 ID：document-1',
    ].join('\n'))
  })

  test('splits files into exact inclusive byte ranges', () => {
    expect(byteRanges(0, 4)).toEqual([])
    expect(byteRanges(1, 4)).toEqual([{ start: 0, end: 0 }])
    expect(byteRanges(10, 4)).toEqual([
      { start: 0, end: 3 },
      { start: 4, end: 7 },
      { start: 8, end: 9 },
    ])
    expect(() => byteRanges(-1, 4)).toThrow()
    expect(() => byteRanges(10, 0)).toThrow()
  })

  test('builds a conservative presentation outline from explicit slide boundaries', () => {
    expect(parsePresentationSlides('# 封面\n简介\n---\n# 架构\n内容')).toEqual([
      { title: '封面', content: '# 封面\n简介' },
      { title: '架构', content: '# 架构\n内容' },
    ])
    expect(parsePresentationSlides('前言\n\n# Slide 1 现状\n内容\n# Slide 2 方案\n内容')).toHaveLength(2)
    expect(parsePresentationSlides('# 封面\n内容\n<!-- page break -->\n# 第二页\n内容')).toHaveLength(2)
    expect(parsePresentationSlides('## 封面\n内容\n## 第二页\n内容')).toHaveLength(2)
    expect(parsePresentationSlides('# 单页\n正文')).toEqual([{ title: '单页', content: '# 单页\n正文' }])
    expect(parsePresentationSlides('')).toEqual([])
  })

  test('maps common code extensions to syntax languages', () => {
    expect(codeLanguage('component.tsx')).toBe('tsx')
    expect(codeLanguage('script.py')).toBe('python')
    expect(codeLanguage('unknown.custom')).toBe('text')
  })

  test('preserves delimiters and newlines inside quoted cells', () => {
    expect(parseDelimitedPreview('name,note\nFox,"line 1\nline 2, still quoted"', ',').rows).toEqual([
      ['name', 'note'],
      ['Fox', 'line 1\nline 2, still quoted'],
    ])
  })

  test('unescapes double quotes', () => {
    expect(parseDelimitedPreview('name\n"Fox ""Runtime"""', ',').rows[1]).toEqual(['Fox "Runtime"'])
  })

  test('caps hostile tables', () => {
    const manyRows = Array.from({ length: MAX_TABLE_ROWS + 20 }, (_, index) => String(index)).join('\n')
    const rowResult = parseDelimitedPreview(manyRows, ',')
    expect(rowResult.rows).toHaveLength(MAX_TABLE_ROWS)
    expect(rowResult.overflow).toBe(true)

    const wideResult = parseDelimitedPreview(Array.from({ length: MAX_TABLE_COLUMNS + 5 }, () => 'x').join(','), ',')
    expect(wideResult.rows[0]).toHaveLength(MAX_TABLE_COLUMNS)
    expect(wideResult.overflow).toBe(true)
  })

  test('formats valid JSON and preserves invalid JSON', () => {
    expect(formatJsonPreview('{"fox":true}')).toBe('{\n  "fox": true\n}')
    expect(formatJsonPreview('{broken')).toBe('{broken')
  })

  test('virtualizes only the visible table window with overscan', () => {
    const top = virtualRowWindow(500, 0, 340)
    expect(top.start).toBe(0)
    expect(top.end).toBeLessThan(40)
    expect(top.afterHeight).toBe((500 - top.end) * TABLE_ROW_HEIGHT)

    const middle = virtualRowWindow(500, 170 * TABLE_ROW_HEIGHT, 340)
    expect(middle.start).toBeGreaterThan(150)
    expect(middle.end - middle.start).toBeLessThan(40)
    expect(middle.beforeHeight + (middle.end - middle.start) * TABLE_ROW_HEIGHT + middle.afterHeight).toBe(500 * TABLE_ROW_HEIGHT)
  })

  test('clamps PDF page and zoom navigation to valid bounds', () => {
    expect(normalizePdfViewport({ page: 99, pageCount: 4, zoom: 10 })).toEqual({ page: 4, pageCount: 4, zoom: MAX_PDF_ZOOM })
    expect(normalizePdfViewport({ page: -2, pageCount: 0, zoom: 0 })).toEqual({ page: 1, pageCount: 1, zoom: MIN_PDF_ZOOM })
    expect(movePdfPage({ page: 2, pageCount: 4, zoom: 1 }, -8).page).toBe(1)
    expect(zoomPdf({ page: 1, pageCount: 1, zoom: 1 }, .25).zoom).toBe(1.25)
  })

  test('rejects suspicious Office archive expansion before parsing', () => {
    const bytes = new Uint8Array([
      0x50, 0x4b, 0x03, 0x04,
      0x50, 0x4b, 0x01, 0x02,
      ...new Uint8Array(42),
      0x50, 0x4b, 0x05, 0x06,
      0, 0, 0, 0,
      2, 0, 2, 0,
      92, 0, 0, 0,
      4, 0, 0, 0,
      0, 0,
    ])
    expect(() => assertSafeZipArchive(bytes, {
      maxEntries: 1,
      maxUncompressedBytes: 1024 * 1024,
      maxEntryUncompressedBytes: 1024 * 1024,
      maxCompressionRatio: 200,
    })).toThrow('过多内部文件')
  })

  test('normalizes precise source locators across runtime metadata variants', () => {
    expect(normalizeKnowledgeSourceLocator({
      id: 'chunk-1',
      excerpt: '  cited text  ',
      metadata: { page_number: '7', section: ' Runtime boundary ' },
    })).toEqual({ chunkId: 'chunk-1', page: 7, anchor: 'Runtime boundary', excerpt: 'cited text' })
    expect(normalizeKnowledgeSourceLocator({ metadata: { page_num: 0 } }, 'fallback')).toEqual({
      chunkId: 'fallback', page: undefined, anchor: undefined, excerpt: undefined,
    })
    expect(normalizeKnowledgeSourceLocator({ page: 'not-a-page', anchor: true })).toEqual({
      chunkId: undefined, page: undefined, anchor: 'true', excerpt: undefined,
    })
  })
})
