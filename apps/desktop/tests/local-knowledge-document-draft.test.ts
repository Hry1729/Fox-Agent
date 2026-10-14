import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { MAX_COMPOSER_ATTACHMENT_BYTES, localKnowledgeDocumentFile } from '../src/features/chat/local-knowledge-document-draft'
import { knowledgeReferenceKey } from '../src/features/conversations/api/desktop-client'
import type { LocalKnowledgeDocumentDto } from '../src/features/conversations/api/desktop-client'
import type { LocalKnowledgeDocumentAsk } from '../src/features/workspace/types'

const payload: LocalKnowledgeDocumentAsk = { knowledgeBaseId: 'kb-a', knowledgeBaseName: '制度', documentId: 'doc-a', documentName: '制度.md' }
const source: LocalKnowledgeDocumentDto = { id: 'doc-a', knowledgeBaseId: 'kb-a', displayName: '制度.md', relativePath: '制度.md', currentRevision: 2, fileSize: 3, mimeType: 'text/markdown', parseStatus: 'ready', indexStatus: 'ready', chunkCount: 1, lastErrorCode: null, lastErrorMessage: null, createdAt: 1, updatedAt: 2 }

describe('a local document becomes a real draft attachment', () => {
  test('uses both Host identities and the current source name, type, and bytes', async () => {
    const reads: unknown[][] = []
    const file = await localKnowledgeDocumentFile({ ...payload, documentName: 'stale.md' }, {
      listLocalKnowledgeDocuments: async () => [source],
      readLocalKnowledgeDocumentFileRange: async (...args) => { reads.push(args); return [65, 66, 67] },
    }, new AbortController().signal)
    expect(reads).toEqual([['kb-a', 'doc-a', 0, 2]])
    expect(file.name).toBe('制度.md')
    expect(file.type).toBe('text/markdown')
    expect(await file.text()).toBe('ABC')
  })

  test('rejects an ID from another library without reading any bytes', async () => {
    let reads = 0
    await expect(localKnowledgeDocumentFile(payload, {
      listLocalKnowledgeDocuments: async () => [{ ...source, knowledgeBaseId: 'kb-b' }],
      readLocalKnowledgeDocumentFileRange: async () => { reads++; return [] },
    }, new AbortController().signal)).rejects.toThrow('已不存在')
    expect(reads).toBe(0)
  })

  test('rejects oversize material before allocating or reading it', async () => {
    let reads = 0
    await expect(localKnowledgeDocumentFile(payload, {
      listLocalKnowledgeDocuments: async () => [{ ...source, fileSize: MAX_COMPOSER_ATTACHMENT_BYTES + 1 }],
      readLocalKnowledgeDocumentFileRange: async () => { reads++; return [] },
    }, new AbortController().signal)).rejects.toThrow('单个附件不能超过 5 MB')
    expect(reads).toBe(0)
  })

  test('reads a five MB file in Host-compatible ranges', async () => {
    const ranges: number[][] = []
    const file = await localKnowledgeDocumentFile(payload, {
      listLocalKnowledgeDocuments: async () => [{ ...source, fileSize: MAX_COMPOSER_ATTACHMENT_BYTES }],
      readLocalKnowledgeDocumentFileRange: async (_kb, _doc, start, end) => { ranges.push([start, end]); return new Uint8Array(end - start + 1) },
    }, new AbortController().signal)
    expect(ranges).toEqual([[0, 4 * 1024 * 1024 - 1], [4 * 1024 * 1024, MAX_COMPOSER_ATTACHMENT_BYTES - 1]])
    expect(file.size).toBe(MAX_COMPOSER_ATTACHMENT_BYTES)
  })

  test('rejects a source revision changed while reading', async () => {
    let lists = 0
    await expect(localKnowledgeDocumentFile(payload, {
      listLocalKnowledgeDocuments: async () => [{ ...source, currentRevision: ++lists }],
      readLocalKnowledgeDocumentFileRange: async () => [65, 66, 67],
    }, new AbortController().signal)).rejects.toThrow('资料已更新')
  })

  test('rejects partial bytes instead of inventing an attachment', async () => {
    await expect(localKnowledgeDocumentFile(payload, {
      listLocalKnowledgeDocuments: async () => [source],
      readLocalKnowledgeDocumentFileRange: async () => [65],
    }, new AbortController().signal)).rejects.toThrow('读取不完整')
  })
})

const workbench = readFileSync(new URL('../src/features/chat/workbench.tsx', import.meta.url), 'utf8')
const callbackSource = workbench.slice(workbench.indexOf('  const askLocalKnowledgeDocument ='), workbench.indexOf('  const switchAssistantMode ='))

function callbackHarness(readFile: typeof localKnowledgeDocumentFile) {
  const request = { current: { version: 0, abort: null as AbortController | null } }
  const events: string[] = []
  const oldConversation = { id: 'existing', references: [{ source: 'local', id: 'existing-kb' }] }
  let selected: string | null = oldConversation.id
  let attachment: File | null = null
  let references: unknown
  const cancel = () => { request.current.version++; request.current.abort?.abort(); request.current.abort = null }
  const env = {
    localAssistantAgent: () => ({ id: 'fox-general' }),
    desktopConversation: {
      enabled: true, error: null,
      createConversationDraftWithKnowledge: (_agentId: string, value: unknown) => { events.push('create', 'bind'); selected = null; references = value; return true },
    },
    toast: { error: (message: string) => events.push(`error:${message}`) },
    cancelDocumentAsk: cancel,
    documentAskRequest: request,
    localKnowledgeDocumentFile: readFile,
    desktopClient: {},
    knowledgeReferenceKey,
    beginNewDraft: () => { events.push('open-chat'); cancel() },
    localKnowledgeDocumentPrompt: () => '【资料：制度.md】\n',
    setComposerInitialAttachments: (value: { files: File[] }) => { attachment = value.files[0] },
    desktopErrorDetails: (cause: Error) => ({ message: cause.message }),
  }
  const compiled = new Bun.Transpiler({ loader: 'ts' }).transformSync(callbackSource)
  const ask = new Function(...Object.keys(env), `${compiled}; return askLocalKnowledgeDocument`)(...Object.values(env)) as (payload: LocalKnowledgeDocumentAsk) => Promise<void>
  return { ask, cancel, events, oldConversation, get selected() { return selected }, get attachment() { return attachment }, get references() { return references } }
}

describe('the reader opens an isolated conversation draft', () => {
  test('opens a fresh unsent draft with the original file and library binding', async () => {
    const file = new File(['ABC'], '制度.md', { type: 'text/markdown' })
    const h = callbackHarness(async () => file)
    await h.ask(payload)
    expect(h.selected).toBeNull()
    expect(h.attachment).toBe(file)
    expect(h.references).toEqual([{ source: 'local', providerKey: 'local', id: 'kb-a' }])
    expect(h.oldConversation.references).toEqual([{ source: 'local', id: 'existing-kb' }])
    expect(h.events).toEqual(['create', 'bind', 'open-chat'])
  })

  test('navigation cancels a delayed source without touching the selected conversation', async () => {
    let resolve!: (file: File) => void
    const h = callbackHarness(async () => new Promise((done) => { resolve = done }))
    const pending = h.ask(payload)
    h.cancel()
    resolve(new File(['ABC'], '制度.md'))
    await pending
    expect(h.selected).toBe('existing')
    expect(h.attachment).toBeNull()
    expect(h.events).toEqual([])
  })

  test('only the newest repeated click creates a draft', async () => {
    const resolvers: Array<(file: File) => void> = []
    const h = callbackHarness(async () => new Promise((done) => resolvers.push(done)))
    const first = h.ask(payload)
    const second = h.ask(payload)
    const newest = new File(['NEW'], 'new.md')
    resolvers[1](newest)
    await second
    resolvers[0](new File(['OLD'], 'old.md'))
    await first
    expect(h.attachment).toBe(newest)
    expect(h.events).toEqual(['create', 'bind', 'open-chat'])
  })

  test('Host read failure leaves the previous conversation selected', async () => {
    const h = callbackHarness(async () => { throw new Error('文件不可读') })
    await h.ask(payload)
    expect(h.selected).toBe('existing')
    expect(h.events).toEqual(['error:文件不可读'])
  })
})
