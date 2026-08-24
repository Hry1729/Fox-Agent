import { useCallback, useEffect, useRef, useState, type WheelEvent } from 'react'
import { ChevronDown, ChevronLeft, ChevronRight, ChevronUp, LoaderCircle, Minus, Plus, Search, X } from 'lucide-react'
import { Input } from '@/components/ui/input'
import type { KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'
import type { KnowledgeSourceLocator } from '@/features/workspace/types'
import type { DocumentViewPosition } from './document-activity-model'
import { MAX_PDF_ZOOM, MIN_PDF_ZOOM, PDF_ZOOM_STEP, movePdfPage, normalizePdfViewport, zoomPdf } from './knowledge-preview-model'
import { openKnowledgeCachedPreviewSource, type KnowledgeCachedPreviewOpener } from './knowledge-preview-source'
import { PreviewIconButton, PreviewToolbar } from './knowledge-preview-controls'
import pdfWorkerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
import type { PDFDocumentLoadingTask, PDFDocumentProxy, RenderTask } from 'pdfjs-dist'

const MAX_PDF_BYTES = 128 * 1024 * 1024
const PDF_SEARCH_LIMIT = 500

interface PdfSearchMatch {
  page: number
  excerpt: string
}

export default function KnowledgePdfViewer({
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
  const canvasHostRef = useRef<HTMLDivElement | null>(null)
  const documentRef = useRef<PDFDocumentProxy | null>(null)
  const renderTaskRef = useRef<RenderTask | null>(null)
  const renderQueueRef = useRef<Promise<void>>(Promise.resolve())
  const renderGenerationRef = useRef(0)
  const scrollRootRef = useRef<HTMLDivElement | null>(null)
  const pendingScrollRef = useRef<'start' | 'end' | null>(null)
  const wheelDeltaRef = useRef(0)
  const wheelResetRef = useRef<number | null>(null)
  const lastWheelPageChangeRef = useRef(0)
  const searchGenerationRef = useRef(0)
  const requestedPageRef = useRef(sourceLocator?.page)
  const locatedSourceRef = useRef('')
  const [viewport, setViewport] = useState(() => normalizePdfViewport({}))
  const [loading, setLoading] = useState(true)
  const [rendering, setRendering] = useState(false)
  const locatorQuery = locatorSearchText(sourceLocator)
  const [query, setQuery] = useState(locatorQuery)
  const [searching, setSearching] = useState(false)
  const [matches, setMatches] = useState<PdfSearchMatch[]>([])
  const [activeMatch, setActiveMatch] = useState(-1)

  useEffect(() => {
    let disposed = false
    let loadingTask: PDFDocumentLoadingTask | null = null
    let source: Awaited<ReturnType<typeof openKnowledgeCachedPreviewSource>> | null = null
    const abortController = new AbortController()
    const load = async () => {
      try {
        const pdfjs = await import('pdfjs-dist')
        pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerUrl
        source = openPreviewSource
          ? await openPreviewSource({ maxBytes: MAX_PDF_BYTES, signal: abortController.signal })
          : await openKnowledgeCachedPreviewSource({
              knowledgeBaseId,
              documentId,
              filename,
              maxBytes: MAX_PDF_BYTES,
              metadata,
              signal: abortController.signal,
            })
        const bytes = await source.readAll()
        if (disposed) return
        loadingTask = pdfjs.getDocument({
          data: bytes,
          stopAtErrors: false,
          enableXfa: false,
          isOffscreenCanvasSupported: false,
          isImageDecoderSupported: false,
        })
        const pdf = await loadingTask.promise
        if (disposed) {
          await loadingTask.destroy()
          return
        }
        documentRef.current = pdf
        setViewport((current) => normalizePdfViewport({ ...current, page: requestedPageRef.current ?? 1, pageCount: pdf.numPages }))
        setLoading(false)
      } catch (cause) {
        if (!disposed) onFailure(errorMessage(cause))
      }
    }
    void load()
    return () => {
      disposed = true
      abortController.abort()
      renderGenerationRef.current += 1
      searchGenerationRef.current += 1
      renderTaskRef.current?.cancel()
      documentRef.current = null
      if (wheelResetRef.current != null) window.clearTimeout(wheelResetRef.current)
      void loadingTask?.destroy()
      void source?.close()
    }
  }, [documentId, filename, knowledgeBaseId, metadata, onFailure, openPreviewSource])

  useEffect(() => {
    requestedPageRef.current = sourceLocator?.page
    setQuery(locatorQuery)
    setMatches([])
    setActiveMatch(-1)
    if (sourceLocator?.page) {
      setViewport((current) => normalizePdfViewport({ ...current, page: sourceLocator.page }))
    }
  }, [locatorQuery, sourceLocator?.page])

  useEffect(() => {
    if (!requestedPosition?.page) return
    setViewport((current) => {
      const next = normalizePdfViewport({
        ...current,
        page: requestedPosition.page,
        zoom: requestedPosition.zoom ?? current.zoom,
      })
      return next.page === current.page && next.zoom === current.zoom ? current : next
    })
  }, [requestedPosition?.page, requestedPosition?.zoom])

  useEffect(() => {
    if (loading) return
    onPositionChange?.({ page: viewport.page, zoom: viewport.zoom })
  }, [loading, onPositionChange, viewport.page, viewport.zoom])

  useEffect(() => {
    const pdf = documentRef.current
    const canvasHost = canvasHostRef.current
    if (!pdf || !canvasHost || loading) return
    const generation = ++renderGenerationRef.current
    renderTaskRef.current?.cancel()
    const render = async () => {
      if (generation !== renderGenerationRef.current) return
      setRendering(true)
      try {
        const page = await pdf.getPage(viewport.page)
        if (generation !== renderGenerationRef.current) {
          return
        }
        const pageViewport = page.getViewport({ scale: viewport.zoom })
        const outputScale = Math.min(window.devicePixelRatio || 1, 2.5)
        // Render into a fresh canvas and only swap it into the viewport after
        // PDF.js has completed. WebView canvas reuse after cancellation can
        // otherwise leave later pages visually blank despite a resolved task.
        const canvas = document.createElement('canvas')
        canvas.setAttribute('aria-label', `${filename} 第 ${viewport.page} 页`)
        canvas.width = Math.floor(pageViewport.width * outputScale)
        canvas.height = Math.floor(pageViewport.height * outputScale)
        canvas.style.width = `${Math.floor(pageViewport.width)}px`
        canvas.style.height = `${Math.floor(pageViewport.height)}px`
        const canvasContext = canvas.getContext('2d', { alpha: false })
        if (!canvasContext) throw new Error('无法创建 PDF 页面画布。')
        const task = page.render({
          canvas,
          canvasContext,
          viewport: pageViewport,
          transform: outputScale === 1 ? undefined : [outputScale, 0, 0, outputScale, 0, 0],
          background: '#ffffff',
        })
        renderTaskRef.current = task
        await task.promise
        if (generation !== renderGenerationRef.current) return
        canvasHost.replaceChildren(canvas)
      } catch (cause) {
        if (generation === renderGenerationRef.current && !(cause instanceof Error && cause.name === 'RenderingCancelledException')) onFailure(errorMessage(cause))
      } finally {
        if (generation === renderGenerationRef.current) {
          renderTaskRef.current = null
          setRendering(false)
          const target = pendingScrollRef.current
          pendingScrollRef.current = null
          if (target) window.requestAnimationFrame(() => {
            const root = scrollRootRef.current
            if (root) root.scrollTop = target === 'end' ? root.scrollHeight : 0
          })
        }
      }
    }
    renderQueueRef.current = renderQueueRef.current.catch(() => undefined).then(render)
    return () => {
      if (renderGenerationRef.current === generation) renderGenerationRef.current += 1
      renderTaskRef.current?.cancel()
    }
  }, [loading, onFailure, viewport.page, viewport.zoom])

  const navigateToPage = useCallback((page: number) => {
    const next = normalizePdfViewport({ ...viewport, page })
    if (next.page === viewport.page) return
    pendingScrollRef.current = next.page < viewport.page ? 'end' : 'start'
    setViewport(next)
  }, [viewport])
  const movePage = (offset: number) => navigateToPage(movePdfPage(viewport, offset).page)
  const changeZoom = (offset: number) => setViewport((current) => zoomPdf(current, offset))
  const runSearch = useCallback(async (requestedQuery?: string) => {
    const pdf = documentRef.current
    const target = (requestedQuery ?? query).trim().toLocaleLowerCase()
    const generation = ++searchGenerationRef.current
    if (!pdf || !target) {
      setMatches([])
      setActiveMatch(-1)
      setSearching(false)
      return
    }
    setSearching(true)
    try {
      const next: PdfSearchMatch[] = []
      for (let pageNumber = 1; pageNumber <= pdf.numPages && next.length < PDF_SEARCH_LIMIT; pageNumber += 1) {
        if (searchGenerationRef.current !== generation) return
        const page = await pdf.getPage(pageNumber)
        if (searchGenerationRef.current !== generation) {
          return
        }
        const content = await page.getTextContent()
        if (searchGenerationRef.current !== generation) {
          return
        }
        const text = content.items.map((item) => ('str' in item ? item.str : '')).join(' ').replace(/\s+/g, ' ').trim()
        const lower = text.toLocaleLowerCase()
        let cursor = 0
        while (cursor <= lower.length - target.length && next.length < PDF_SEARCH_LIMIT) {
          const index = lower.indexOf(target, cursor)
          if (index < 0) break
          next.push({ page: pageNumber, excerpt: text.slice(Math.max(0, index - 36), Math.min(text.length, index + target.length + 54)) })
          cursor = index + Math.max(1, target.length)
        }
      }
      if (searchGenerationRef.current !== generation) return
      setMatches(next)
      setActiveMatch(next.length ? 0 : -1)
      if (next[0]) navigateToPage(next[0].page)
    } catch (cause) {
      if (searchGenerationRef.current === generation) onFailure(errorMessage(cause))
    } finally {
      if (searchGenerationRef.current === generation) setSearching(false)
    }
  }, [navigateToPage, onFailure, query])
  useEffect(() => {
    if (loading || !locatorQuery || sourceLocator?.page) return
    const locatorKey = `${documentId}:${sourceLocator?.chunkId ?? ''}:${sourceLocator?.page ?? ''}:${locatorQuery}`
    if (locatedSourceRef.current === locatorKey) return
    locatedSourceRef.current = locatorKey
    setQuery(locatorQuery)
    void runSearch(locatorQuery)
  }, [documentId, loading, locatorQuery, runSearch, sourceLocator?.chunkId, sourceLocator?.page])
  const moveMatch = (offset: number) => {
    if (!matches.length) return
    const next = (activeMatch + offset + matches.length) % matches.length
    setActiveMatch(next)
    navigateToPage(matches[next].page)
  }
  const handlePageWheel = useCallback((event: WheelEvent<HTMLDivElement>) => {
    if (rendering || Math.abs(event.deltaY) < Math.abs(event.deltaX)) return
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
    pendingScrollRef.current = direction > 0 ? 'start' : 'end'
    setViewport((current) => movePdfPage(current, direction))
  }, [rendering])

  return (
    <div className="fox-preview-surface fox-pdf-viewer">
      <PreviewToolbar>
        <span className="fox-pdf-page-navigation" aria-label="页面导航">
          <PreviewIconButton label="上一页" disabled={viewport.page <= 1} onClick={() => movePage(-1)}><ChevronLeft /></PreviewIconButton>
          <label className="fox-pdf-page-field">
            <input aria-label="跳转页码" type="number" min={1} max={viewport.pageCount} value={viewport.page} onChange={(event) => navigateToPage(Number(event.target.value))} />
            <span>/ {viewport.pageCount}</span>
          </label>
          <PreviewIconButton label="下一页" disabled={viewport.page >= viewport.pageCount} onClick={() => movePage(1)}><ChevronRight /></PreviewIconButton>
        </span>
        <label className="fox-preview-search">
          <Search />
          <Input
            aria-label="在 PDF 中搜索"
            placeholder="搜索 PDF"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => { if (event.key === 'Enter') void runSearch() }}
          />
          {query && <button type="button" aria-label="清除搜索" onClick={() => { searchGenerationRef.current += 1; setSearching(false); setQuery(''); setMatches([]); setActiveMatch(-1) }}><X /></button>}
        </label>
        <span className="fox-preview-toolbar-label">
          {searching ? '搜索中' : matches.length ? `${Math.max(0, activeMatch) + 1} / ${matches.length} 处` : `${viewport.page} / ${viewport.pageCount} 页`}
        </span>
        <PreviewIconButton label="上一个搜索结果" disabled={!matches.length || searching} onClick={() => moveMatch(-1)}><ChevronUp /></PreviewIconButton>
        <PreviewIconButton label="下一个搜索结果" disabled={!matches.length || searching} onClick={() => moveMatch(1)}><ChevronDown /></PreviewIconButton>
        <PreviewIconButton label="缩小" disabled={viewport.zoom <= MIN_PDF_ZOOM} onClick={() => changeZoom(-PDF_ZOOM_STEP)}><Minus /></PreviewIconButton>
        <span className="fox-pdf-zoom-label">{Math.round(viewport.zoom * 100)}%</span>
        <PreviewIconButton label="放大" disabled={viewport.zoom >= MAX_PDF_ZOOM} onClick={() => changeZoom(PDF_ZOOM_STEP)}><Plus /></PreviewIconButton>
      </PreviewToolbar>
      {matches[activeMatch] && <p className="fox-pdf-search-excerpt">第 {matches[activeMatch].page} 页 · {matches[activeMatch].excerpt}</p>}
      <div ref={scrollRootRef} className="fox-pdf-canvas-viewport is-single-page" onWheel={handlePageWheel}>
        {(loading || rendering) && <span className="fox-pdf-rendering"><LoaderCircle className="animate-spin" />{loading ? '正在打开 PDF' : `正在渲染第 ${viewport.page} 页`}</span>}
        <div ref={canvasHostRef} className="fox-pdf-canvas-host" />
        {!loading && <small className="fox-pdf-page-number">第 {viewport.page} / {viewport.pageCount} 页 · 上下滚动切换页面</small>}
      </div>
    </div>
  )
}

function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

function locatorSearchText(locator?: KnowledgeSourceLocator | null) {
  const value = (locator?.anchor || locator?.excerpt || '').replace(/\s+/g, ' ').trim()
  return value.length > 80 ? value.slice(0, 80).trim() : value
}
