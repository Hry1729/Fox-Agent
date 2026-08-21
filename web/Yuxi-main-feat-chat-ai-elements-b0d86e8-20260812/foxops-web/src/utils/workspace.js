import * as XLSX from 'xlsx'

const MARKDOWN_EXTENSIONS = new Set(['md', 'markdown', 'mdx'])
const IMAGE_EXTENSIONS = new Set([
  'apng',
  'avif',
  'png',
  'jpg',
  'jpeg',
  'gif',
  'bmp',
  'webp',
  'svg'
])
const TEXT_EXTENSIONS = new Set([
  'txt',
  'text',
  'log',
  'json',
  'jsonl',
  'yaml',
  'yml',
  'toml',
  'ini',
  'cfg',
  'conf',
  'csv',
  'tsv',
  'py',
  'js',
  'ts',
  'jsx',
  'tsx',
  'vue',
  'css',
  'less',
  'scss',
  'xml',
  'sql',
  'sh',
  'env',
  'gitignore'
])
const SPREADSHEET_EXTENSIONS = new Set(['xls', 'xlsx', 'xlsm', 'xlsb', 'ods'])
const OFFICE_EXTENSIONS = new Set(['doc', 'docx', 'ppt', 'pptx'])
const MAX_SPREADSHEET_ROWS = 200
const MAX_SPREADSHEET_COLUMNS = 50

export function buildQuery(params = {}) {
  const query = new URLSearchParams()
  Object.entries(params).forEach(([key, value]) => {
    if (value !== undefined && value !== null && value !== '') {
      query.set(key, String(value))
    }
  })
  return query.toString()
}

export function normalizeWorkspacePath(path) {
  const value = String(path || '/')
    .trim()
    .replace(/\\/g, '/')
  const absolute = value.startsWith('/') ? value : `/${value}`
  return absolute.replace(/\/{2,}/g, '/').replace(/\/$/, '') || '/'
}

export function buildBreadcrumbs(path, rootLabel = '工作区') {
  const normalized = normalizeWorkspacePath(path)
  const breadcrumbs = [{ name: rootLabel, path: '/' }]
  if (normalized === '/') return breadcrumbs

  normalized
    .split('/')
    .filter(Boolean)
    .forEach((name) => {
      const parentPath = breadcrumbs.at(-1).path
      breadcrumbs.push({ name, path: parentPath === '/' ? `/${name}` : `${parentPath}/${name}` })
    })
  return breadcrumbs
}

export function formatFileSize(bytes) {
  if (bytes === 0 || bytes === '0') return '0 B'
  if (bytes === undefined || bytes === null || bytes === '') return '-'

  const value = Number(bytes)
  if (!Number.isFinite(value) || value < 0) return '-'
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB']
  const unitIndex = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1)
  const amount = Number((value / 1024 ** unitIndex).toFixed(2))
  return `${amount} ${units[unitIndex]}`
}

function extensionOf(path) {
  const name =
    String(path || '')
      .toLowerCase()
      .split(/[?#]/)[0]
      .split('/')
      .pop() || ''
  const dotIndex = name.lastIndexOf('.')
  return dotIndex > 0 ? name.slice(dotIndex + 1) : ''
}

export function getPreviewType(path, contentType = '') {
  const mime = String(contentType).toLowerCase()
  if (mime.includes('application/pdf')) return 'pdf'
  if (mime.startsWith('image/')) return 'image'
  if (
    mime.includes('spreadsheet') ||
    mime.includes('ms-excel') ||
    mime.includes('opendocument.spreadsheet')
  ) {
    return 'spreadsheet'
  }
  if (mime.includes('text/markdown')) return 'markdown'
  if (mime.includes('text/html')) return 'html'
  if (mime.startsWith('text/') || mime.includes('application/json')) return 'text'

  const extension = extensionOf(path)
  if (MARKDOWN_EXTENSIONS.has(extension)) return 'markdown'
  if (IMAGE_EXTENSIONS.has(extension)) return 'image'
  if (extension === 'pdf') return 'pdf'
  if (extension === 'html' || extension === 'htm') return 'html'
  if (SPREADSHEET_EXTENSIONS.has(extension)) return 'spreadsheet'
  if (OFFICE_EXTENSIONS.has(extension)) return 'office'
  if (TEXT_EXTENSIONS.has(extension)) return 'text'
  return 'unsupported'
}

const normalizeSpreadsheetCell = (value) => {
  if (value === undefined || value === null) return ''
  if (typeof value === 'object') return JSON.stringify(value)
  return String(value)
}

/**
 * 将 Excel/ODS 文件转换为受限的只读表格数据，避免把超大工作簿完整挂到 DOM。
 */
export async function parseSpreadsheetBlob(blob) {
  const workbook = XLSX.read(await blob.arrayBuffer(), {
    type: 'array',
    cellDates: true,
    dense: true
  })

  const sheets = workbook.SheetNames.map((name) => {
    const sheet = workbook.Sheets[name]
    const reference = sheet?.['!ref']
    if (!reference) {
      return { name, rows: [], totalRows: 0, totalColumns: 0, truncated: false }
    }

    const sourceRange = XLSX.utils.decode_range(reference)
    const totalRows = Math.max(0, sourceRange.e.r - sourceRange.s.r + 1)
    const totalColumns = Math.max(0, sourceRange.e.c - sourceRange.s.c + 1)
    const previewRange = {
      s: sourceRange.s,
      e: {
        r: Math.min(sourceRange.e.r, sourceRange.s.r + MAX_SPREADSHEET_ROWS - 1),
        c: Math.min(sourceRange.e.c, sourceRange.s.c + MAX_SPREADSHEET_COLUMNS - 1)
      }
    }
    const rows = XLSX.utils
      .sheet_to_json(sheet, {
        header: 1,
        raw: false,
        defval: '',
        blankrows: true,
        range: previewRange
      })
      .map((row) => row.map(normalizeSpreadsheetCell))

    return {
      name,
      rows,
      totalRows,
      totalColumns,
      truncated: totalRows > MAX_SPREADSHEET_ROWS || totalColumns > MAX_SPREADSHEET_COLUMNS
    }
  })

  return {
    sheets,
    truncated: sheets.some((sheet) => sheet.truncated),
    limits: { rows: MAX_SPREADSHEET_ROWS, columns: MAX_SPREADSHEET_COLUMNS }
  }
}

export function parseDownloadFilename(contentDisposition) {
  const header = String(contentDisposition || '')
  const utf8Match = header.match(/filename\*=UTF-8''([^;]+)/i)
  if (utf8Match?.[1]) {
    try {
      return decodeURIComponent(utf8Match[1])
    } catch {
      return utf8Match[1]
    }
  }

  return header.match(/filename="?([^";]+)"?/i)?.[1] || ''
}

/**
 * 将工作区/知识库预览响应归一化为统一预览对象。
 * 文本类预览通常返回 JSON；图片/PDF 等返回二进制流。
 */
export async function normalizePreviewResponse(response, baseFile = {}) {
  const contentType = response?.headers?.get?.('content-type') || ''

  if (contentType.includes('application/json')) {
    const payload = await response.json()
    const previewType = payload.preview_type || payload.previewType || payload.kind || 'text'
    return {
      ...baseFile,
      ...payload,
      name: baseFile.name || payload.filename || payload.name,
      content: payload.content ?? '',
      previewType,
      supported: payload.supported !== false,
      message: payload.message || '',
      previewUrl: ''
    }
  }

  const headerPreviewType = response?.headers?.get?.('x-yuxi-preview-type')
  const previewType =
    headerPreviewType || getPreviewType(baseFile.path || baseFile.name, contentType)
  const blob = await response.blob()

  if (previewType === 'spreadsheet') {
    try {
      return {
        ...baseFile,
        content: null,
        previewType,
        supported: true,
        message: '',
        previewUrl: '',
        spreadsheet: await parseSpreadsheetBlob(blob)
      }
    } catch (error) {
      return {
        ...baseFile,
        content: null,
        previewType: 'unsupported',
        supported: false,
        message: `表格文件解析失败：${error?.message || '文件格式异常'}`,
        previewUrl: ''
      }
    }
  }

  if (['markdown', 'text', 'html'].includes(previewType)) {
    return {
      ...baseFile,
      content: await blob.text(),
      previewType,
      supported: true,
      message: '',
      previewUrl: ''
    }
  }

  if (previewType === 'office') {
    return {
      ...baseFile,
      content: null,
      previewType,
      supported: false,
      message: 'Word/PowerPoint 暂不支持浏览器内预览，请下载后查看',
      previewUrl: ''
    }
  }

  const createObjectURL = globalThis.URL?.createObjectURL
  const previewUrl =
    typeof createObjectURL === 'function' ? createObjectURL.call(globalThis.URL, blob) : ''

  return {
    ...baseFile,
    content: null,
    previewType,
    supported: previewType !== 'unsupported',
    message: previewType === 'unsupported' ? '当前文件暂不支持预览，请下载后查看' : '',
    previewUrl
  }
}
