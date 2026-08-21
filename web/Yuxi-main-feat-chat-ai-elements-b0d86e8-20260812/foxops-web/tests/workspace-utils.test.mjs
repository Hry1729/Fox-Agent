import test from 'node:test'
import assert from 'node:assert/strict'
import * as XLSX from 'xlsx'

import {
  buildBreadcrumbs,
  buildQuery,
  formatFileSize,
  getPreviewType,
  normalizePreviewResponse,
  normalizeWorkspacePath,
  parseDownloadFilename
} from '../src/utils/workspace.js'

test('buildQuery keeps false and zero while omitting empty values', () => {
  assert.equal(
    buildQuery({ path: '/', recursive: false, page: 0, empty: '', nil: null, missing: undefined }),
    'path=%2F&recursive=false&page=0'
  )
})

test('normalizeWorkspacePath produces an absolute path without trailing slash', () => {
  assert.equal(normalizeWorkspacePath(''), '/')
  assert.equal(normalizeWorkspacePath('agents/'), '/agents')
  assert.equal(normalizeWorkspacePath('/saved_artifacts///'), '/saved_artifacts')
})

test('buildBreadcrumbs includes the root and every nested directory', () => {
  assert.deepEqual(buildBreadcrumbs('/', '工作区'), [{ name: '工作区', path: '/' }])
  assert.deepEqual(buildBreadcrumbs('/agents/report', '工作区'), [
    { name: '工作区', path: '/' },
    { name: 'agents', path: '/agents' },
    { name: 'report', path: '/agents/report' }
  ])
})

test('formatFileSize handles zero, missing and binary units', () => {
  assert.equal(formatFileSize(0), '0 B')
  assert.equal(formatFileSize(null), '-')
  assert.equal(formatFileSize(1024), '1 KB')
  assert.equal(formatFileSize(1536), '1.5 KB')
})

test('getPreviewType resolves path and content type consistently', () => {
  assert.equal(getPreviewType('README.md'), 'markdown')
  assert.equal(getPreviewType('image.png'), 'image')
  assert.equal(getPreviewType('manual.pdf'), 'pdf')
  assert.equal(getPreviewType('index.html'), 'html')
  assert.equal(getPreviewType('slides.pptx'), 'office')
  assert.equal(getPreviewType('inspection.xlsx'), 'spreadsheet')
  assert.equal(
    getPreviewType(
      'unknown.bin',
      'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet'
    ),
    'spreadsheet'
  )
  assert.equal(getPreviewType('config.yaml'), 'text')
  assert.equal(getPreviewType('unknown.bin', 'image/webp'), 'image')
  assert.equal(getPreviewType('unknown.bin'), 'unsupported')
})

test('normalizePreviewResponse parses XLSX sheets into a bounded table preview', async () => {
  const workbook = XLSX.utils.book_new()
  XLSX.utils.book_append_sheet(
    workbook,
    XLSX.utils.aoa_to_sheet([
      ['故障码', '含义'],
      ['E101', '编码器异常']
    ]),
    '故障表'
  )
  const bytes = XLSX.write(workbook, { type: 'array', bookType: 'xlsx' })
  const response = {
    headers: {
      get: (name) =>
        name === 'content-type'
          ? 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet'
          : null
    },
    blob: async () => new Blob([bytes])
  }

  const preview = await normalizePreviewResponse(response, { name: 'faults.xlsx' })
  assert.equal(preview.previewType, 'spreadsheet')
  assert.equal(preview.supported, true)
  assert.equal(preview.previewUrl, '')
  assert.equal(preview.spreadsheet.sheets[0].name, '故障表')
  assert.deepEqual(preview.spreadsheet.sheets[0].rows[1], ['E101', '编码器异常'])
})

test('parseDownloadFilename prefers UTF-8 filename and supports quoted fallback', () => {
  assert.equal(
    parseDownloadFilename("attachment; filename*=UTF-8''%E5%B7%A5%E4%BD%9C%E5%8C%BA.md"),
    '工作区.md'
  )
  assert.equal(parseDownloadFilename('attachment; filename="report.txt"'), 'report.txt')
  assert.equal(parseDownloadFilename(''), '')
})

test('normalizePreviewResponse parses JSON text preview and binary streams', async () => {
  const jsonResponse = {
    headers: { get: (name) => (name === 'content-type' ? 'application/json' : null) },
    json: async () => ({
      content: '# hello',
      preview_type: 'markdown',
      supported: true
    })
  }
  const jsonFile = await normalizePreviewResponse(jsonResponse, {
    name: 'README.md',
    path: '/README.md'
  })
  assert.equal(jsonFile.previewType, 'markdown')
  assert.equal(jsonFile.content, '# hello')
  assert.equal(jsonFile.previewUrl, '')

  const originalCreate = globalThis.URL.createObjectURL
  Object.defineProperty(globalThis.URL, 'createObjectURL', {
    configurable: true,
    writable: true,
    value: () => 'blob:preview'
  })
  try {
    const binaryResponse = {
      headers: {
        get: (name) => {
          if (name === 'content-type') return 'application/pdf'
          if (name === 'x-yuxi-preview-type') return 'pdf'
          return null
        }
      },
      blob: async () => new Blob(['%PDF'], { type: 'application/pdf' })
    }
    const binaryFile = await normalizePreviewResponse(binaryResponse, {
      name: 'a.pdf',
      path: '/a.pdf'
    })
    assert.equal(binaryFile.previewType, 'pdf')
    assert.equal(binaryFile.previewUrl, 'blob:preview')
    assert.equal(binaryFile.content, null)
  } finally {
    Object.defineProperty(globalThis.URL, 'createObjectURL', {
      configurable: true,
      writable: true,
      value: originalCreate
    })
  }
})
