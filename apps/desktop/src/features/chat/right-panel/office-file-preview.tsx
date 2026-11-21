import { lazy, Suspense, useCallback, useState } from 'react'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { binaryValue, type WholeFilePreviewOpener } from '@/features/knowledge/knowledge-preview-source'
import { Button } from '@/components/ui/button'

const DocxViewer = lazy(() => import('@/features/knowledge/knowledge-docx-viewer'))
const SpreadsheetViewer = lazy(() => import('@/features/knowledge/knowledge-spreadsheet-viewer'))

export function officePreviewKind(filename: string) {
  if (/\.docx$/iu.test(filename)) return 'docx'
  if (/\.xlsx$/iu.test(filename)) return 'xlsx'
  return null
}

type OfficeSource = Parameters<typeof desktopClient.readConversationOfficePreview>[1]

export function OfficeFilePreview({ conversationId, source, filename }: {
  conversationId: string
  source: OfficeSource
  filename: string
}) {
  const [error, setError] = useState<string | null>(null)
  const [attempt, setAttempt] = useState(0)
  const kind = source.kind
  const id = source.kind === 'artifact' ? source.artifactId : source.path
  const openPreviewSource = useCallback<WholeFilePreviewOpener>(async ({ maxBytes, signal }) => ({
    async readAll() {
      signal?.throwIfAborted()
      const source: OfficeSource = kind === 'artifact' ? { kind, artifactId: id } : { kind, path: id }
      const bytes = binaryValue(await desktopClient.readConversationOfficePreview(conversationId, source))
      signal?.throwIfAborted()
      if (bytes.byteLength > maxBytes) throw new Error('文件超过侧栏预览大小上限')
      return bytes
    },
    async close() {},
  }), [conversationId, kind, id])
  if (error) return <div className="fox-file-preview-state" role="alert">
    <strong>文件没有打开</strong><span>{error}</span>
    <Button size="sm" variant="outline" onClick={() => { setError(null); setAttempt(value => value + 1) }}>重试</Button>
  </div>
  const Viewer = officePreviewKind(filename) === 'docx' ? DocxViewer : SpreadsheetViewer
  return <div className="fox-office-file-preview">
    <Suspense fallback={<p role="status">正在加载文件预览…</p>}>
      <Viewer key={attempt} knowledgeBaseId="conversation-files" documentId={id} filename={filename} onFailure={setError} openPreviewSource={openPreviewSource} />
    </Suspense>
  </div>
}
