import { describe, expect, test } from 'bun:test'
import { attachmentSourceNames, stepExtraSource } from '../src/features/chat/step-source-summary'

/**
 * The extra source is evidence-driven: every case below feeds the shape the real
 * tool call uses, and the cases that have no source must return '' rather than a
 * guess. Nothing here asserts CSS.
 */
describe('process step extra source', () => {
  test('a worksheet comes from the read result, not from the arguments', () => {
    expect(stepExtraSource('read', { path: '6-1.xlsx' }, { sheets: [{ name: 'Detailed Sales' }, { name: 'Daily Summary' }] }))
      .toBe('Detailed Sales')
    // A read with no sheet directory carries no extra source.
    expect(stepExtraSource('read', { path: 'notes.md' }, { text: 'hello' })).toBe('')
  })

  test('an attachment source is the workbook name, never an internal artifact id', () => {
    expect(stepExtraSource('attachment_compute', { artifactIds: ['compute-artifact:4969408f'] }, { attachments: [{ id: 'project:6-1.xlsx', name: '6-1.xlsx' }] }))
      .toBe('6-1.xlsx')
    expect(stepExtraSource('attachment_compute', { projectPaths: ['6-1.xlsx'] }, {})).toBe('6-1.xlsx')
    // Only an internal artifact id: no source is claimed.
    expect(stepExtraSource('attachment_compute', { artifactIds: ['compute-artifact:4969408f'] }, {})).toBe('')
  })

  test('a web source is the page title when present and the host otherwise', () => {
    expect(stepExtraSource('web_search', { query: 'agv' }, { results: [{ title: 'AGV 调度综述', url: 'https://example.com/a' }] }))
      .toBe('AGV 调度综述')
    expect(stepExtraSource('http_request', { url: 'https://docs.example.com/page' }, {})).toBe('docs.example.com')
    expect(stepExtraSource('web_read', {}, {})).toBe('')
  })

  test('a directory call names the project root only when no path was given', () => {
    expect(stepExtraSource('ls', {}, {})).toBe('项目根目录')
    // With a path the row already renders it, so no second copy is added.
    expect(stepExtraSource('ls', { path: 'in' }, {})).toBe('')
  })

  test('arguments, code and diagnostics never become a source', () => {
    const call = { code: 'const wb = attachments[0]; return {rows: wb.sheets.length}', command: 'pnpm exec tsc --noEmit' }
    expect(stepExtraSource('attachment_compute', call, {})).toBe('')
    expect(stepExtraSource('run_command', call, {})).toBe('')
    expect(stepExtraSource('mystery_tool', { path: 'x.txt' }, {})).toBe('')
  })

  test('an unnamed result body, an id list and a diagnostic body are never a source', () => {
    // Every entry here is the shape a real (but unnamed) payload has: headers and
    // bodies the row must not paraphrase into a source.
    const noisy = {
      attachments: [{ id: 'compute-artifact:4969408f', code: 'rows.map(row => row[0])', message: 'budget exceeded' }],
      sheets: [{ id: 'sheet-1' }, { index: 2 }],
      details: { code: 'compute_budget_exceeded', message: 'budget 120000ms exceeded' },
    }
    expect(stepExtraSource('attachment_compute', {}, noisy)).toBe('')
    expect(stepExtraSource('read', { path: '6-1.xlsx' }, noisy)).toBe('')
    expect(stepExtraSource('read_attachment', noisy, noisy)).toBe('')
    // A URL that is not a URL is not a host, and an unknown tool never invents one.
    expect(stepExtraSource('http_request', {}, { results: [{ url: 'notaurl' }] })).toBe('')
    expect(stepExtraSource('mystery_tool', {}, { title: '不应出现', results: [{ title: '也不应出现' }] })).toBe('')
    // The one thing that does count — a project path the caller itself named — stays.
    expect(stepExtraSource('attachment_compute', { projectPaths: ['6-1.xlsx'] }, noisy)).toBe('6-1.xlsx')
  })

  test('an attachment id resolves only through the conversation name table', () => {
    const names = new Map([['e2c8b6a4-1f00-4c2a-9b3d-0f9e8d7c6b5a', '6-1.xlsx']])
    // A real upload: the uuid is replaced by the name the conversation shows.
    expect(attachmentSourceNames({ attachmentIds: ['e2c8b6a4-1f00-4c2a-9b3d-0f9e8d7c6b5a'] }, names)).toBe('6-1.xlsx')
    // Unmapped values are never guessed from appearance, however file-like they look.
    expect(attachmentSourceNames({ attachmentIds: ['6-1.xlsx'] }, new Map())).toBe('')
    expect(attachmentSourceNames({ attachmentIds: ['e2c8b6a4-1f00-4c2a-9b3d-0f9e8d7c6b5a'] }, new Map())).toBe('')
    expect(attachmentSourceNames({ attachmentIds: ['compute-artifact:4969408f41561445'] }, new Map())).toBe('')
    expect(attachmentSourceNames({ attachmentIds: ['notanid'] }, new Map())).toBe('')
    // No table at all means nothing to resolve against.
    expect(attachmentSourceNames({ attachmentIds: ['6-1.xlsx'] }, undefined)).toBe('')
    // A mapped name is trimmed and returned even when the key looks nothing like a file.
    expect(attachmentSourceNames({ attachmentIds: ['abc'] }, new Map([['abc', '  sales.xlsx  ']]))).toBe('sales.xlsx')
    // The generic source extractor never falls back to the raw id either.
    expect(stepExtraSource('attachment_compute', { attachmentIds: ['e2c8b6a4-1f00-4c2a-9b3d-0f9e8d7c6b5a'] }, {})).toBe('')
  })
})
