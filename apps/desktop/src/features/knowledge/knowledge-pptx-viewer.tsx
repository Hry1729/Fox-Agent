import { useEffect, useRef, useState } from 'react'
import { ChevronLeft, ChevronRight, LoaderCircle } from 'lucide-react'
import type { KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'
import type { DocumentViewPosition } from './document-activity-model'
import { assertSafeZipArchive } from './knowledge-archive-safety'
import { PreviewIconButton, PreviewToolbar } from './knowledge-preview-controls'
import { openKnowledgeCachedPreviewSource, type KnowledgeCachedPreviewOpener } from './knowledge-preview-source'

const MAX_PPTX_BYTES = 64 * 1024 * 1024
const PPTX_ZIP_LIMITS = {
  maxEntries: 4_000,
  maxUncompressedBytes: 256 * 1024 * 1024,
  maxEntryUncompressedBytes: 128 * 1024 * 1024,
  maxCompressionRatio: 200,
}
const RENDERER_ZIP_LIMITS = {
  maxEntries: 4_000,
  maxEntryUncompressedBytes: 128 * 1024 * 1024,
  maxTotalUncompressedBytes: 256 * 1024 * 1024,
  maxMediaBytes: 192 * 1024 * 1024,
  maxConcurrency: 4,
}

interface PptxViewerInstance {
  slideCount: number
  currentSlideIndex: number
  goToSlide(index: number): Promise<void>
  destroy(): void
}

export default function KnowledgePptxViewer({
  knowledgeBaseId,
  documentId,
  filename,
  metadata,
  requestedPosition,
  onPositionChange,
  onFailure,
  openPreviewSource,
}: {
  knowledgeBaseId: string
  documentId: string
  filename: string
  metadata: KnowledgeDocumentSourceMetadata
  requestedPosition?: DocumentViewPosition
  onPositionChange?(position: DocumentViewPosition): void
  onFailure(message: string): void
  openPreviewSource?: KnowledgeCachedPreviewOpener
}) {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const viewerRef = useRef<PptxViewerInstance | null>(null)
  const [loading, setLoading] = useState(true)
  const [navigating, setNavigating] = useState(false)
  const [slideIndex, setSlideIndex] = useState(0)
  const [slideCount, setSlideCount] = useState(0)

  useEffect(() => {
    let disposed = false
    const abortController = new AbortController()

    const load = async () => {
      let source: Awaited<ReturnType<typeof openKnowledgeCachedPreviewSource>> | null = null
      try {
        setLoading(true)
        setSlideIndex(0)
        setSlideCount(0)
        const [{ PptxViewer }, nextSource] = await Promise.all([
          import('@aiden0z/pptx-renderer'),
          openPreviewSource
            ? openPreviewSource({ maxBytes: MAX_PPTX_BYTES, signal: abortController.signal })
            : openKnowledgeCachedPreviewSource({
                knowledgeBaseId,
                documentId,
                filename,
                maxBytes: MAX_PPTX_BYTES,
                metadata,
                signal: abortController.signal,
              }),
        ])
        source = nextSource
        const bytes = await source.readAll()
        assertSafeZipArchive(bytes, PPTX_ZIP_LIMITS)
        if (disposed || !containerRef.current) return

        containerRef.current.replaceChildren()
        const viewer = await PptxViewer.open(bytes.buffer as ArrayBuffer, containerRef.current, {
          renderMode: 'slide',
          fitMode: 'contain',
          zipLimits: RENDERER_ZIP_LIMITS,
          lazySlides: true,
          lazyMedia: true,
          pdfjs: false,
          signal: abortController.signal,
          onSlideChange: (index) => {
            if (!disposed) setSlideIndex(index)
          },
        })
        if (disposed) {
          viewer.destroy()
          return
        }
        viewerRef.current = viewer
        setSlideCount(viewer.slideCount)
        const requestedSlide = Math.min(viewer.slideCount - 1, Math.max(0, (requestedPosition?.page ?? 1) - 1))
        if (requestedSlide !== viewer.currentSlideIndex) await viewer.goToSlide(requestedSlide)
        setSlideIndex(viewer.currentSlideIndex)
        setLoading(false)
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
      viewerRef.current?.destroy()
      viewerRef.current = null
      containerRef.current?.replaceChildren()
    }
  }, [documentId, filename, knowledgeBaseId, metadata, onFailure, openPreviewSource])

  useEffect(() => {
    const viewer = viewerRef.current
    if (!viewer || loading || !slideCount || !requestedPosition?.page) return
    const next = Math.min(slideCount - 1, Math.max(0, requestedPosition.page - 1))
    if (next === viewer.currentSlideIndex) return
    void viewer.goToSlide(next).then(() => setSlideIndex(viewer.currentSlideIndex)).catch((cause) => {
      onFailure(cause instanceof Error ? cause.message : String(cause))
    })
  }, [loading, onFailure, requestedPosition?.page, slideCount])

  useEffect(() => {
    if (!loading && slideCount) onPositionChange?.({ page: slideIndex + 1 })
  }, [loading, onPositionChange, slideCount, slideIndex])

  const moveSlide = async (offset: number) => {
    const viewer = viewerRef.current
    if (!viewer || navigating) return
    const next = Math.min(slideCount - 1, Math.max(0, slideIndex + offset))
    if (next === slideIndex) return
    try {
      setNavigating(true)
      await viewer.goToSlide(next)
      setSlideIndex(viewer.currentSlideIndex)
    } catch (cause) {
      onFailure(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setNavigating(false)
    }
  }

  return (
    <div className="fox-pptx-preview-shell">
      <div className="fox-preview-surface fox-pptx-preview-surface">
        <PreviewToolbar>
          <span className="fox-preview-toolbar-label">{slideCount ? `第 ${slideIndex + 1} / ${slideCount} 页` : '正在读取幻灯片'}</span>
          <PreviewIconButton label="上一页" disabled={loading || navigating || slideIndex <= 0} onClick={() => void moveSlide(-1)}><ChevronLeft /></PreviewIconButton>
          <PreviewIconButton label="下一页" disabled={loading || navigating || slideIndex >= slideCount - 1} onClick={() => void moveSlide(1)}><ChevronRight /></PreviewIconButton>
        </PreviewToolbar>
        <div ref={containerRef} className="fox-pptx-preview" aria-label={`${filename} 只读演示文稿预览`} />
        {(loading || navigating) && <span className="fox-office-preview-loading"><LoaderCircle className="animate-spin" />{loading ? '正在渲染演示文稿' : '正在切换幻灯片'}</span>}
      </div>
    </div>
  )
}
