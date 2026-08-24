import { useCallback, useEffect, useRef, useState, type WheelEvent } from 'react'
import { ChevronLeft, ChevronRight, LoaderCircle } from 'lucide-react'
import type { KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'
import type { KnowledgeSourceLocator } from '@/features/workspace/types'
import type { DocumentViewPosition } from './document-activity-model'
import { assertSafeZipArchive } from './knowledge-archive-safety'
import { PreviewIconButton, PreviewToolbar } from './knowledge-preview-controls'
import { openKnowledgeCachedPreviewSource, type KnowledgeCachedPreviewOpener } from './knowledge-preview-source'

const MAX_DOCX_BYTES = 64 * 1024 * 1024
const DOCX_ZIP_LIMITS = {
  maxEntries: 5_000,
  maxUncompressedBytes: 256 * 1024 * 1024,
  maxEntryUncompressedBytes: 128 * 1024 * 1024,
  maxCompressionRatio: 200,
}

export default function KnowledgeDocxViewer({
  knowledgeBaseId,
  documentId,
  filename,
  metadata,
  sourceLocator,
  requestedPosition,
  onPositionChange,
  onFailure,
  openPreviewSource,
}: {
  knowledgeBaseId: string
  documentId: string
  filename: string
  metadata: KnowledgeDocumentSourceMetadata
  sourceLocator?: KnowledgeSourceLocator | null
  requestedPosition?: DocumentViewPosition
  onPositionChange?(position: DocumentViewPosition): void
  onFailure(message: string): void
  openPreviewSource?: KnowledgeCachedPreviewOpener
}) {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const wheelDeltaRef = useRef(0)
  const wheelResetRef = useRef<number | null>(null)
  const lastWheelPageChangeRef = useRef(0)
  const [loading, setLoading] = useState(true)
  const [page, setPage] = useState(1)
  const [pageCount, setPageCount] = useState(0)
  const continuous = pageCount === 1

  const showPage = useCallback((requestedPage: number) => {
    const pages = docxPages(containerRef.current)
    if (!pages.length) return
    if (pages.length === 1) {
      const onlyPage = pages[0]
      onlyPage.classList.add('is-active', 'is-continuous')
      onlyPage.hidden = false
      onlyPage.style.setProperty('display', 'block', 'important')
      onlyPage.setAttribute('aria-hidden', 'false')
      setPage(1)
      return
    }
    const next = Math.min(pages.length, Math.max(1, requestedPage))
    pages.forEach((element, index) => {
      const active = index === next - 1
      element.classList.toggle('is-active', active)
      element.classList.remove('is-continuous')
      element.hidden = !active
      element.style.setProperty('display', active ? 'block' : 'none', 'important')
      element.setAttribute('aria-hidden', String(!active))
    })
    setPage(next)
    containerRef.current?.scrollTo({ top: 0 })
  }, [])

  useEffect(() => {
    let disposed = false
    const abortController = new AbortController()
    const load = async () => {
      let source: Awaited<ReturnType<typeof openKnowledgeCachedPreviewSource>> | null = null
      try {
        setLoading(true)
        const [{ renderAsync }, nextSource] = await Promise.all([
          import('docx-preview'),
          openPreviewSource
            ? openPreviewSource({ maxBytes: MAX_DOCX_BYTES, signal: abortController.signal })
            : openKnowledgeCachedPreviewSource({
                knowledgeBaseId,
                documentId,
                filename,
                maxBytes: MAX_DOCX_BYTES,
                metadata,
                signal: abortController.signal,
              }),
        ])
        source = nextSource
        const bytes = await source.readAll()
        assertSafeZipArchive(bytes, DOCX_ZIP_LIMITS)
        if (disposed || !containerRef.current) return
        containerRef.current.replaceChildren()
        await renderAsync(bytes, containerRef.current, undefined, {
          inWrapper: true,
          breakPages: true,
          renderHeaders: true,
          renderFooters: true,
          renderFootnotes: true,
          renderEndnotes: true,
          renderComments: false,
          renderChanges: false,
          renderAltChunks: false,
          ignoreWidth: false,
          ignoreHeight: false,
          useBase64URL: true,
          debug: false,
        })
        if (disposed || !containerRef.current) return
        const pages = docxPages(containerRef.current)
        setPageCount(pages.length)
        let initialPage = clampPage(sourceLocator?.page, pages.length)
        const located = locateRenderedText(
          containerRef.current,
          [sourceLocator?.anchor, sourceLocator?.excerpt].filter(Boolean).join('\n'),
        )
        if (located) {
          located.classList.add('fox-source-text-highlight')
          const locatedPage = located.closest<HTMLElement>('section.docx')
          const locatedIndex = pages.indexOf(locatedPage ?? pages[0])
          if (locatedIndex >= 0) initialPage = locatedIndex + 1
        }
        showPage(initialPage)
        if (located) window.requestAnimationFrame(() => scrollLocatedTextIntoView(containerRef.current, located))
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
      if (wheelResetRef.current != null) window.clearTimeout(wheelResetRef.current)
      containerRef.current?.replaceChildren()
    }
  }, [documentId, filename, knowledgeBaseId, metadata, onFailure, openPreviewSource, showPage, sourceLocator?.anchor, sourceLocator?.excerpt, sourceLocator?.page])

  useEffect(() => {
    if (loading || !requestedPosition?.page || requestedPosition.page === page) return
    showPage(requestedPosition.page)
  }, [loading, page, requestedPosition?.page, showPage])

  useEffect(() => {
    if (loading) return
    onPositionChange?.({ page })
  }, [loading, onPositionChange, page])

  const handlePageWheel = (event: WheelEvent<HTMLDivElement>) => {
    if (loading || continuous || Math.abs(event.deltaY) < Math.abs(event.deltaX)) return
    const root = event.currentTarget
    const atStart = root.scrollTop <= 2
    const atEnd = root.scrollTop + root.clientHeight >= root.scrollHeight - 2
    if ((event.deltaY < 0 && !atStart) || (event.deltaY > 0 && !atEnd)) return
    event.preventDefault()
    wheelDeltaRef.current += event.deltaY
    if (wheelResetRef.current != null) window.clearTimeout(wheelResetRef.current)
    wheelResetRef.current = window.setTimeout(() => { wheelDeltaRef.current = 0 }, 180)
    const now = performance.now()
    if (Math.abs(wheelDeltaRef.current) < 72 || now - lastWheelPageChangeRef.current < 360) return
    const direction = wheelDeltaRef.current > 0 ? 1 : -1
    wheelDeltaRef.current = 0
    lastWheelPageChangeRef.current = now
    showPage(page + direction)
  }

  return (
    <div className="fox-docx-preview-shell">
      <div className="fox-preview-surface fox-docx-preview-surface">
        <PreviewToolbar>
          <span className="fox-preview-toolbar-label">{pageCount ? continuous ? '连续文档' : `第 ${page} / ${pageCount} 页` : '正在读取文档页面'}</span>
          {!continuous && <PreviewIconButton label="上一页" disabled={loading || page <= 1} onClick={() => showPage(page - 1)}><ChevronLeft /></PreviewIconButton>}
          {!continuous && <PreviewIconButton label="下一页" disabled={loading || page >= pageCount} onClick={() => showPage(page + 1)}><ChevronRight /></PreviewIconButton>}
        </PreviewToolbar>
        <div
          ref={containerRef}
          className="fox-docx-preview"
          onWheel={handlePageWheel}
          onClick={(event) => {
            const anchor = (event.target as HTMLElement).closest('a')
            if (anchor) event.preventDefault()
          }}
        />
      </div>
      {loading && <span className="fox-office-preview-loading"><LoaderCircle className="animate-spin" />正在渲染 Word 文档</span>}
    </div>
  )
}

function docxPages(root: HTMLElement | null) {
  return root ? Array.from(root.querySelectorAll<HTMLElement>('section.docx')) : []
}

function locateRenderedText(root: HTMLElement, source: string) {
  const queries = locatorQueries(source)
  if (!queries.length) return null
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  const nodes: Array<{ node: Text; start: number; end: number }> = []
  let documentText = ''
  let current = walker.nextNode()
  while (current) {
    const value = normalizeLocatorText(current.textContent ?? '')
    if (value) {
      if (documentText) documentText += ' '
      const start = documentText.length
      documentText += value
      nodes.push({ node: current as Text, start, end: documentText.length })
    }
    current = walker.nextNode()
  }
  for (const query of queries) {
    const index = documentText.indexOf(query)
    if (index < 0) continue
    return nodes.find((item) => item.end > index)?.node.parentElement ?? null
  }
  return null
}

function locatorQueries(source: string) {
  const normalized = normalizeLocatorText(source)
  if (!normalized) return []
  const lines = source.split(/\r?\n/).map(normalizeLocatorText).filter((item) => item.length >= 8)
  const seeds = [...lines, normalized]
  const lengths = [120, 80, 48, 28, 16]
  return [...new Set(seeds.flatMap((seed) => lengths.filter((length) => seed.length >= length).map((length) => seed.slice(0, length))))]
    .sort((left, right) => right.length - left.length)
}

function normalizeLocatorText(value: string) {
  return value.replace(/\s+/g, ' ').trim().toLocaleLowerCase()
}

function clampPage(page: number | undefined, pageCount: number) {
  if (!page || pageCount <= 0) return 1
  return Math.min(pageCount, Math.max(1, page))
}

function scrollLocatedTextIntoView(root: HTMLElement | null, target: HTMLElement) {
  if (!root || target.closest<HTMLElement>('section.docx')?.hidden) return
  const rootBounds = root.getBoundingClientRect()
  const targetBounds = target.getBoundingClientRect()
  root.scrollTo({
    top: Math.max(0, root.scrollTop + targetBounds.top - rootBounds.top - root.clientHeight * .28),
    behavior: 'smooth',
  })
}
