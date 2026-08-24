import { useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { LoaderCircle } from 'lucide-react'
import type { KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'
import { assertSafeZipArchive, hasZipSignature } from './knowledge-archive-safety'
import { openKnowledgeCachedPreviewSource, type KnowledgeCachedPreviewOpener } from './knowledge-preview-source'

const MAX_SPREADSHEET_BYTES = 64 * 1024 * 1024
const WORKBOOK_ZIP_LIMITS = {
  maxEntries: 10_000,
  maxUncompressedBytes: 512 * 1024 * 1024,
  maxEntryUncompressedBytes: 256 * 1024 * 1024,
  maxCompressionRatio: 200,
}

interface ExcelPreviewInstance {
  preview(source: ArrayBuffer): Promise<unknown>
  destroy(): void
}

export default function KnowledgeSpreadsheetViewer({
  knowledgeBaseId,
  documentId,
  filename,
  metadata,
  onFailure,
  openPreviewSource,
}: {
  knowledgeBaseId: string
  documentId: string
  filename: string
  metadata: KnowledgeDocumentSourceMetadata
  onFailure(message: string): void
  openPreviewSource?: KnowledgeCachedPreviewOpener
}) {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    let disposed = false
    let preview: ExcelPreviewInstance | null = null
    const abortController = new AbortController()

    const load = async () => {
      let source: Awaited<ReturnType<typeof openKnowledgeCachedPreviewSource>> | null = null
      try {
        setLoading(true)
        source = openPreviewSource
          ? await openPreviewSource({ maxBytes: MAX_SPREADSHEET_BYTES, signal: abortController.signal })
          : await openKnowledgeCachedPreviewSource({
              knowledgeBaseId,
              documentId,
              filename,
              maxBytes: MAX_SPREADSHEET_BYTES,
              metadata,
              signal: abortController.signal,
            })
        const bytes = await source.readAll()
        if (hasZipSignature(bytes)) assertSafeZipArchive(bytes, WORKBOOK_ZIP_LIMITS)
        if (disposed || !containerRef.current) return

        const [{ default: jsPreviewExcel }] = await Promise.all([
          import('@js-preview/excel'),
          import('@js-preview/excel/lib/index.css'),
        ])
        if (disposed || !containerRef.current) return

        containerRef.current.replaceChildren()
        preview = jsPreviewExcel.init(containerRef.current, { showContextmenu: false })
        await preview.preview(bytes.buffer as ArrayBuffer)
        if (!disposed) setLoading(false)
      } catch (cause) {
        if (!disposed && !(cause instanceof DOMException && cause.name === 'AbortError')) {
          onFailure(cause instanceof Error ? cause.message : String(cause))
        }
      } finally {
        await source?.close()
      }
    }

    void load()
    return () => {
      disposed = true
      abortController.abort()
      preview?.destroy()
      preview = null
      containerRef.current?.replaceChildren()
    }
  }, [documentId, filename, knowledgeBaseId, metadata, onFailure, openPreviewSource])

  return (
    <div className="fox-spreadsheet-preview-shell">
      <div
        ref={containerRef}
        className="fox-spreadsheet-preview"
        aria-label={`${filename} 只读电子表格预览`}
        onContextMenu={(event) => event.preventDefault()}
        onDoubleClickCapture={(event) => event.preventDefault()}
        onKeyDownCapture={(event) => {
          if (isSpreadsheetEditKey(event)) event.preventDefault()
        }}
        onCutCapture={(event) => event.preventDefault()}
        onPasteCapture={(event) => event.preventDefault()}
      />
      {loading && <span className="fox-office-preview-loading"><LoaderCircle className="animate-spin" />正在渲染电子表格</span>}
    </div>
  )
}

function isSpreadsheetEditKey(event: KeyboardEvent<HTMLElement>) {
  if ((event.ctrlKey || event.metaKey) && ['v', 'x'].includes(event.key.toLowerCase())) return true
  if (event.ctrlKey || event.metaKey || event.altKey) return false
  return event.key.length === 1 || ['Backspace', 'Delete', 'F2'].includes(event.key)
}
