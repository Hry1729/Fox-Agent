export type PreviewKind =
  | 'image'
  | 'markdown'
  | 'json'
  | 'text'
  | 'code'
  | 'csv'
  | 'pdf'
  | 'docx'
  | 'spreadsheet'
  | 'presentation'
  | 'legacy-office'
  | 'unsupported'

export type PreviewSourceStrategy = 'range' | 'cache' | 'direct'

export interface KnowledgeDocumentInfo {
  knowledgeBase: string
  filename: string
  kind: string
  size: string
  updatedAt: string
  status: string
  documentId: string
}

export function formatKnowledgeDocumentInfo(info: KnowledgeDocumentInfo) {
  return [
    `知识库：${info.knowledgeBase}`,
    `文件名：${info.filename}`,
    `类型：${info.kind}`,
    `大小：${info.size}`,
    `更新时间：${info.updatedAt}`,
    `处理状态：${info.status}`,
    `文档 ID：${info.documentId}`,
  ].join('\n')
}

export function previewSourceStrategy(kind: PreviewKind): PreviewSourceStrategy {
  if (kind === 'pdf' || kind === 'image' || kind === 'docx' || kind === 'spreadsheet' || kind === 'presentation') return 'cache'
  return 'direct'
}

export const MAX_TABLE_ROWS = 500
export const MAX_TABLE_COLUMNS = 100
export const MAX_TABLE_CELL_CHARACTERS = 10_000
export const TABLE_ROW_HEIGHT = 34
export const TABLE_OVERSCAN = 8
export const MAX_PRESENTATION_SLIDES = 500
export const MIN_PDF_ZOOM = .5
export const MAX_PDF_ZOOM = 3
export const PDF_ZOOM_STEP = .25

export interface PresentationSlide {
  title: string
  content: string
}

export interface PdfViewportState {
  page: number
  pageCount: number
  zoom: number
}

export function normalizePdfViewport(state: Partial<PdfViewportState>): PdfViewportState {
  const pageCount = Math.max(1, Math.floor(Number.isFinite(state.pageCount) ? state.pageCount! : 1))
  const page = Math.min(pageCount, Math.max(1, Math.floor(Number.isFinite(state.page) ? state.page! : 1)))
  const zoom = Math.min(MAX_PDF_ZOOM, Math.max(MIN_PDF_ZOOM, Number.isFinite(state.zoom) ? state.zoom! : 1))
  return { page, pageCount, zoom }
}

export function movePdfPage(state: PdfViewportState, offset: number) {
  return normalizePdfViewport({ ...state, page: state.page + offset })
}

export function zoomPdf(state: PdfViewportState, offset: number) {
  return normalizePdfViewport({ ...state, zoom: state.zoom + offset })
}

export function previewKind(filename: string): PreviewKind {
  const extension = filename.split('.').pop()?.toLowerCase() ?? ''
  if (['png', 'jpg', 'jpeg', 'webp', 'gif'].includes(extension)) return 'image'
  if (['md', 'mdx', 'markdown'].includes(extension)) return 'markdown'
  if (extension === 'json') return 'json'
  if (['csv', 'tsv'].includes(extension)) return 'csv'
  if (['txt', 'log', 'ini', 'conf', 'yaml', 'yml', 'toml', 'xml'].includes(extension)) return 'text'
  if (['js', 'jsx', 'ts', 'tsx', 'py', 'rs', 'go', 'java', 'kt', 'c', 'h', 'cpp', 'hpp', 'cs', 'css', 'scss', 'html', 'vue', 'svelte', 'sql', 'sh', 'ps1'].includes(extension)) return 'code'
  if (extension === 'pdf') return 'pdf'
  if (extension === 'docx') return 'docx'
  if (['xls', 'xlsx', 'ods'].includes(extension)) return 'spreadsheet'
  if (['pptx', 'odp'].includes(extension)) return 'presentation'
  if (['doc', 'ppt'].includes(extension)) return 'legacy-office'
  return 'unsupported'
}

export function byteRanges(size: number, chunkSize: number) {
  if (!Number.isSafeInteger(size) || size < 0) throw new Error('File size must be a non-negative safe integer.')
  if (!Number.isSafeInteger(chunkSize) || chunkSize <= 0) throw new Error('Chunk size must be a positive safe integer.')
  const ranges: Array<{ start: number; end: number }> = []
  for (let start = 0; start < size; start += chunkSize) {
    ranges.push({ start, end: Math.min(size - 1, start + chunkSize - 1) })
  }
  return ranges
}

export function formatJsonPreview(value: string) {
  try { return JSON.stringify(JSON.parse(value), null, 2) }
  catch { return value }
}

export function codeLanguage(filename: string) {
  const extension = filename.split('.').pop()?.toLowerCase() ?? 'text'
  return ({
    js: 'javascript', jsx: 'jsx', ts: 'typescript', tsx: 'tsx', py: 'python', rs: 'rust', go: 'go', java: 'java',
    kt: 'kotlin', c: 'c', h: 'c', cpp: 'cpp', hpp: 'cpp', cs: 'csharp', css: 'css', scss: 'scss', html: 'html',
    vue: 'vue', svelte: 'svelte', sql: 'sql', sh: 'shellscript', ps1: 'powershell', json: 'json', md: 'markdown',
  } as Record<string, string>)[extension] ?? 'text'
}

export function parseDelimitedPreview(text: string, delimiter: string) {
  const rows: string[][] = []
  let row: string[] = []
  let cell = ''
  let quoted = false
  let overflow = false

  const pushCell = () => {
    if (row.length < MAX_TABLE_COLUMNS) row.push(cell.slice(0, MAX_TABLE_CELL_CHARACTERS))
    else overflow = true
    cell = ''
  }
  const pushRow = () => {
    pushCell()
    if (row.some(Boolean)) rows.push(row)
    row = []
    if (rows.length >= MAX_TABLE_ROWS) overflow = true
  }

  for (let index = 0; index < text.length; index += 1) {
    const character = text[index]
    if (character === '"') {
      if (quoted && text[index + 1] === '"') {
        if (cell.length < MAX_TABLE_CELL_CHARACTERS) cell += '"'
        index += 1
      } else quoted = !quoted
      continue
    }
    if (character === delimiter && !quoted) {
      pushCell()
      continue
    }
    if ((character === '\n' || character === '\r') && !quoted) {
      if (character === '\r' && text[index + 1] === '\n') index += 1
      pushRow()
      if (rows.length >= MAX_TABLE_ROWS) break
      continue
    }
    if (cell.length < MAX_TABLE_CELL_CHARACTERS) cell += character
    else overflow = true
  }
  if (!overflow || rows.length < MAX_TABLE_ROWS) pushRow()
  if (rows.length > MAX_TABLE_ROWS) rows.length = MAX_TABLE_ROWS
  return { rows, overflow }
}

export function virtualRowWindow(rowCount: number, scrollTop: number, viewportHeight: number) {
  const start = Math.max(0, Math.floor(Math.max(0, scrollTop) / TABLE_ROW_HEIGHT) - TABLE_OVERSCAN)
  const count = Math.ceil(Math.max(0, viewportHeight) / TABLE_ROW_HEIGHT) + TABLE_OVERSCAN * 2
  const end = Math.min(Math.max(0, rowCount), start + count)
  return { start, end, beforeHeight: start * TABLE_ROW_HEIGHT, afterHeight: Math.max(0, rowCount - end) * TABLE_ROW_HEIGHT }
}

export function parsePresentationSlides(markdown: string): PresentationSlide[] {
  const source = markdown.replace(/\r\n?/g, '\n').trim()
  if (!source) return []
  const dividerParts = source.split(/^\s*---+\s*$/gm).map((part) => part.trim()).filter(Boolean)
  if (dividerParts.length > 1) return dividerParts.slice(0, MAX_PRESENTATION_SLIDES).map((part, index) => presentationSlide(part, index))

  const lines = source.split('\n')
  const pageBreakStarts = slideStarts(lines, /^\s*(?:<!--\s*)?(?:page|slide)[-_ ]?break(?:\s*-->)?\s*$/i)
  if (pageBreakStarts.length) return slidesFromBreaks(lines, pageBreakStarts)
  const explicitStarts = slideStarts(lines, /^#{1,3}\s*(?:(?:slide|page)\s*\d+|第\s*[一二三四五六七八九十百零〇两\d]+\s*(?:页|张|部分)|幻灯片\s*\d*)\b/i)
  if (explicitStarts.length > 1) return slidesFromStarts(lines, explicitStarts)
  const headingStarts = slideStarts(lines, /^#{1,2}\s+\S/)
  if (headingStarts.length > 1) return slidesFromStarts(lines, headingStarts)
  return [presentationSlide(source, 0)]
}

function slidesFromBreaks(lines: string[], breaks: number[]) {
  const starts = [0, ...breaks.map((index) => index + 1)]
  const ends = [...breaks, lines.length]
  return starts.map((start, index) => lines.slice(start, ends[index]).join('\n').trim())
    .filter(Boolean)
    .slice(0, MAX_PRESENTATION_SLIDES)
    .map((content, index) => presentationSlide(content, index))
}

function slideStarts(lines: string[], pattern: RegExp) {
  const starts: number[] = []
  lines.forEach((line, index) => { if (pattern.test(line.trim())) starts.push(index) })
  return starts
}

function slidesFromStarts(lines: string[], starts: number[]) {
  const prefix = lines.slice(0, starts[0]).join('\n').trim()
  return starts.slice(0, MAX_PRESENTATION_SLIDES).map((start, index) => {
    const end = starts[index + 1] ?? lines.length
    const content = `${index === 0 && prefix ? `${prefix}\n\n` : ''}${lines.slice(start, end).join('\n')}`.trim()
    return presentationSlide(content, index)
  })
}

function presentationSlide(content: string, index: number): PresentationSlide {
  const heading = content.match(/^#{1,6}\s+(.+)$/m)?.[1]
  const title = heading?.replace(/[*_`~]/g, '').trim() || `第 ${index + 1} 页`
  return { title, content }
}
