import { lazy, Suspense, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { AlertCircle, ChevronDown, ChevronUp, FileQuestion, FileText, LoaderCircle, Maximize2, Minus, Plus, RotateCcw, Search, WrapText, X } from 'lucide-react'
import { MessageResponse } from '@/components/ai-elements/message-response'
import { CodeBlock } from '@/components/ai-elements/code-block'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { KnowledgeDocumentRecord, KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'
import type { KnowledgeSourceLocator } from '@/features/workspace/types'
import { codeLanguage, formatJsonPreview, parseDelimitedPreview, parsePresentationSlides, previewKind, TABLE_ROW_HEIGHT, virtualRowWindow, type PreviewKind } from './knowledge-preview-model'
import { binaryValue, openKnowledgeCachedPreviewSource } from './knowledge-preview-source'
import { PreviewIconButton, PreviewToolbar } from './knowledge-preview-controls'
import { KnowledgeDocumentActivityToolbar } from './knowledge-document-activity-toolbar'
import { compatibleReadingState, type DocumentViewPosition } from './document-activity-model'
import { useDocumentActivity } from './use-document-activity'

const KnowledgePdfViewer = lazy(() => import('./knowledge-pdf-viewer'))
const KnowledgeDocxViewer = lazy(() => import('./knowledge-docx-viewer'))
const KnowledgeSpreadsheetViewer = lazy(() => import('./knowledge-spreadsheet-viewer'))
const KnowledgePptxViewer = lazy(() => import('./knowledge-pptx-viewer'))

type PreviewStatus = 'loading' | 'ready' | 'failed' | 'unsupported'

interface PreviewState {
  status: PreviewStatus
  kind: PreviewKind
  metadata: KnowledgeDocumentSourceMetadata | null
  text: string
  objectUrl: string | null
  truncated: boolean
  fallback: boolean
  error: string | null
}

const EMPTY_STATE: PreviewState = {
  status: 'loading',
  kind: 'unsupported',
  metadata: null,
  text: '',
  objectUrl: null,
  truncated: false,
  fallback: false,
  error: null,
}

const MAX_IMAGE_BYTES = 64 * 1024 * 1024
const MAX_TEXT_PREVIEW_BYTES = 2 * 1024 * 1024
const MIN_IMAGE_ZOOM = 0.25
const MAX_IMAGE_ZOOM = 4
const TEXT_SEARCH_RESULT_LIMIT = 500

export function KnowledgeFileViewer({
  knowledgeBaseId,
  document,
  parsedContent,
  parsedContentLoading,
  sourceLocator,
}: {
  knowledgeBaseId: string
  document: KnowledgeDocumentRecord
  parsedContent: string
  parsedContentLoading: boolean
  sourceLocator?: KnowledgeSourceLocator | null
}) {
  const [attempt, setAttempt] = useState(0)
  const [state, setState] = useState<PreviewState>(EMPTY_STATE)
  const [imageZoom, setImageZoom] = useState(1)
  const [imageFit, setImageFit] = useState(true)
  const [viewPosition, setViewPosition] = useState<DocumentViewPosition>({ page: 1 })
  const requestVersion = useRef(0)
  const restoredRevision = useRef<string | null>(null)
  const activity = useDocumentActivity(knowledgeBaseId, document.id, state.metadata?.sourceRevision ?? null)

  useEffect(() => {
    const version = ++requestVersion.current
    let disposed = false
    let nextObjectUrl: string | null = null
    const abortController = new AbortController()
    setImageZoom(1)
    setImageFit(true)
    setState({ ...EMPTY_STATE, kind: previewKind(document.name) })

    const commit = (next: PreviewState) => {
      if (disposed || requestVersion.current !== version) {
        if (next.objectUrl) URL.revokeObjectURL(next.objectUrl)
        return
      }
      setState((current) => {
        if (current.objectUrl && current.objectUrl !== next.objectUrl) URL.revokeObjectURL(current.objectUrl)
        return next
      })
    }

    const load = async () => {
      const kind = previewKind(document.name)
      if (kind === 'legacy-office' || kind === 'unsupported') {
        commit({
          ...EMPTY_STATE,
          status: 'unsupported',
          kind,
          error: kind === 'legacy-office'
            ? '旧版 Office 二进制格式暂不支持快速预览，请下载原文件查看。'
            : '当前文件格式暂不支持内置预览。',
        })
        return
      }

      try {
        const metadata = await desktopClient.getKnowledgeDocumentSourceMetadata(knowledgeBaseId, document.id)
        if (disposed || requestVersion.current !== version) return
        if (kind === 'presentation' && !document.name.toLowerCase().endsWith('.pptx')) {
          if (parsedContentLoading) return
          if (parsedContent) {
            commit(presentationFallback(parsedContent, metadata))
            return
          }
          commit({
            ...EMPTY_STATE,
            status: 'unsupported',
            kind,
            metadata,
            error: '当前演示文稿格式暂不支持源版式预览，请下载原文件查看。',
          })
          return
        }
        if (kind === 'pdf' || kind === 'docx' || kind === 'spreadsheet' || kind === 'presentation') {
          commit({ ...EMPTY_STATE, status: 'ready', kind, metadata })
          return
        }

        if (kind === 'image') {
          const source = await openKnowledgeCachedPreviewSource({
            knowledgeBaseId,
            documentId: document.id,
            filename: document.name,
            maxBytes: MAX_IMAGE_BYTES,
            metadata,
            signal: abortController.signal,
          })
          let bytes: Uint8Array
          try {
            bytes = await source.readAll()
          } finally {
            await source.close()
          }
          if (disposed || requestVersion.current !== version) return
          nextObjectUrl = URL.createObjectURL(new Blob([bytes.buffer as ArrayBuffer], { type: metadata.mediaType || imageMediaType(document.name) }))
          commit({ ...EMPTY_STATE, status: 'ready', kind, metadata, objectUrl: nextObjectUrl })
          return
        }

        const previewBytes = Math.min(metadata.size, MAX_TEXT_PREVIEW_BYTES)
        const bytes = previewBytes > 0
          ? await readRange(knowledgeBaseId, document.id, 0, previewBytes - 1, metadata.sourceRevision)
          : new Uint8Array()
        if (disposed || requestVersion.current !== version) return
        const decoded = new TextDecoder('utf-8', { fatal: false }).decode(bytes)
        commit({
          ...EMPTY_STATE,
          status: 'ready',
          kind,
          metadata,
          text: kind === 'json' ? formatJsonPreview(decoded) : decoded,
          truncated: metadata.size > previewBytes,
        })
      } catch (cause) {
        const message = cause instanceof Error ? cause.message : String(cause)
        if (parsedContent) {
          commit({ ...parsedFallback(kind, parsedContent), error: message })
        } else {
          commit({ ...EMPTY_STATE, status: 'failed', kind, error: message })
        }
      }
    }

    void load()
    return () => {
      disposed = true
      abortController.abort()
      if (nextObjectUrl) URL.revokeObjectURL(nextObjectUrl)
    }
  }, [attempt, document.id, document.name, knowledgeBaseId, parsedContent, parsedContentLoading])

  const handleRendererFailure = useCallback((message: string) => {
    setState((current) => {
      if (current.status === 'failed') return current
      if (parsedContent && current.kind === 'presentation' && current.metadata) {
        return { ...presentationFallback(parsedContent, current.metadata), error: message }
      }
      if (parsedContent) return { ...parsedFallback(current.kind, parsedContent), error: message }
      return { ...current, status: 'failed', error: message }
    })
  }, [parsedContent])
  const csvRows = useMemo(() => state.kind === 'csv' && state.text ? parseDelimitedPreview(state.text, document.name.endsWith('.tsv') ? '\t' : ',') : null, [document.name, state.kind, state.text])
  useEffect(() => {
    const revisionKey = `${document.id}:${state.metadata?.sourceRevision ?? ''}`
    if (activity.loading || restoredRevision.current === revisionKey) return
    restoredRevision.current = revisionKey
    const saved = compatibleReadingState(activity.activity.readingState, state.metadata?.sourceRevision ?? null)
    setViewPosition(saved
      ? { page: saved.page, scrollOffset: saved.scrollOffset, zoom: saved.zoom }
      : { page: sourceLocator?.page ?? 1 })
  }, [activity.activity.readingState, activity.loading, document.id, sourceLocator?.page, state.metadata?.sourceRevision])
  const updatePosition = useCallback((position: DocumentViewPosition) => {
    const revisionKey = `${document.id}:${state.metadata?.sourceRevision ?? ''}`
    if (activity.loading || restoredRevision.current !== revisionKey) return
    setViewPosition(position)
    activity.savePosition(position)
  }, [activity.loading, activity.savePosition, document.id, state.metadata?.sourceRevision])

  if (state.status === 'loading') {
    return <div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>正在准备预览</b><p>正在读取原文件元数据和所需内容。</p></div>
  }

  if (state.status === 'failed' || state.status === 'unsupported') {
    return <div className="fox-file-preview-state"><FileQuestion /><b>{state.status === 'failed' ? '预览加载失败' : '暂不支持内置预览'}</b><p>{state.error}</p>{state.status === 'failed' && <Button variant="outline" size="sm" onClick={() => setAttempt((value) => value + 1)}><RotateCcw />重试</Button>}</div>
  }

  return (
    <section className="fox-file-preview" data-kind={state.kind}>
      <div className="fox-fast-preview">
        {state.fallback && <div className="fox-file-preview-notice"><AlertCircle /><span><b>当前显示解析内容</b><small>{state.error ? `原件预览暂不可用：${state.error}` : '此视图用于快速阅读，不代表原文件版式。'}</small></span></div>}
        {state.truncated && <div className="fox-file-preview-notice"><FileText /><span><b>已展示前 {formatBytes(MAX_TEXT_PREVIEW_BYTES)}</b><small>为保持页面流畅，较长文件不会一次加载到界面中。</small></span></div>}
        {state.kind === 'image' && state.objectUrl && <ImagePreview filename={document.name} objectUrl={state.objectUrl} zoom={imageZoom} fit={imageFit} onFit={() => { setImageFit(true); setImageZoom(1) }} onZoom={(next) => { setImageFit(false); setImageZoom(Math.min(MAX_IMAGE_ZOOM, Math.max(MIN_IMAGE_ZOOM, next))) }} />}
        {sourceLocator && <SourceLocatorNotice locator={sourceLocator} />}
        <KnowledgeDocumentActivityToolbar activity={activity.activity} sourceRevision={state.metadata?.sourceRevision ?? null} position={viewPosition} loading={activity.loading} error={activity.error} onNavigate={setViewPosition} onAddBookmark={(position) => activity.addBookmark(position)} onRemoveBookmark={async (id) => { const item = activity.activity.bookmarks.find((bookmark) => bookmark.id === id); if (item) await activity.removeBookmark(item) }} onAddAnnotation={(position, note) => activity.addAnnotation(position, note)} onRemoveAnnotation={async (id) => { const item = activity.activity.annotations.find((annotation) => annotation.id === id); if (item) await activity.removeAnnotation(item) }} />
        {state.kind === 'pdf' && state.metadata && <Suspense fallback={<PreviewModuleLoading label="正在加载 PDF 查看器" />}><KnowledgePdfViewer knowledgeBaseId={knowledgeBaseId} documentId={document.id} filename={document.name} metadata={state.metadata} sourceLocator={sourceLocator} requestedPosition={viewPosition} onPositionChange={updatePosition} onFailure={handleRendererFailure} /></Suspense>}
        {state.kind === 'docx' && state.metadata && <Suspense fallback={<PreviewModuleLoading label="正在加载 Word 查看器" />}><KnowledgeDocxViewer knowledgeBaseId={knowledgeBaseId} documentId={document.id} filename={document.name} metadata={state.metadata} sourceLocator={sourceLocator} requestedPosition={viewPosition} onPositionChange={updatePosition} onFailure={handleRendererFailure} /></Suspense>}
        {state.kind === 'spreadsheet' && state.metadata && <Suspense fallback={<PreviewModuleLoading label="正在加载电子表格查看器" />}><KnowledgeSpreadsheetViewer knowledgeBaseId={knowledgeBaseId} documentId={document.id} filename={document.name} metadata={state.metadata} onFailure={handleRendererFailure} /></Suspense>}
        {state.kind === 'presentation' && state.metadata && !state.fallback && <Suspense fallback={<PreviewModuleLoading label="正在加载 PowerPoint 查看器" />}><KnowledgePptxViewer knowledgeBaseId={knowledgeBaseId} documentId={document.id} filename={document.name} metadata={state.metadata} requestedPosition={viewPosition} onPositionChange={updatePosition} onFailure={handleRendererFailure} /></Suspense>}
        {state.kind === 'presentation' && state.fallback && <PresentationPreview markdown={state.text} emptyMessage={state.error} />}
        {(state.kind === 'markdown' || (state.fallback && state.kind !== 'presentation')) && <div className="fox-markdown-preview-card"><MessageResponse className="fox-rich-document-preview">{state.text}</MessageResponse></div>}
        {!state.fallback && state.kind === 'text' && <TextPreview text={state.text} sourceLocator={sourceLocator} />}
        {!state.fallback && (state.kind === 'json' || state.kind === 'code') && <CodeBlock className="fox-source-document-preview" code={state.text} language={codeLanguage(document.name)} showLineNumbers />}
        {!state.fallback && state.kind === 'csv' && csvRows && <DelimitedPreview rows={csvRows.rows} overflow={csvRows.overflow} />}
      </div>
    </section>
  )
}

function parsedFallback(kind: PreviewKind, text: string): PreviewState {
  return {
    ...EMPTY_STATE,
    status: 'ready',
    kind: ['image', 'pdf', 'docx', 'spreadsheet', 'presentation', 'legacy-office'].includes(kind) ? 'markdown' : kind,
    text,
    fallback: true,
  }
}

function presentationFallback(text: string, metadata: KnowledgeDocumentSourceMetadata): PreviewState {
  return {
    ...EMPTY_STATE,
    status: 'ready',
    kind: 'presentation',
    metadata,
    text,
    fallback: true,
    error: '当前显示演示文稿的解析内容，不代表原始幻灯片版式。',
  }
}

async function readRange(knowledgeBaseId: string, documentId: string, start: number, end: number, sourceRevision: string) {
  const value: unknown = await desktopClient.readKnowledgeDocumentRange(knowledgeBaseId, documentId, start, end, sourceRevision)
  return binaryValue(value)
}

function imageMediaType(filename: string) {
  const extension = filename.split('.').pop()?.toLowerCase()
  return extension === 'jpg' || extension === 'jpeg' ? 'image/jpeg' : `image/${extension || 'png'}`
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${Math.round(bytes / 1024 / 1024)} MB`
}

function ImagePreview({ filename, objectUrl, zoom, fit, onFit, onZoom }: { filename: string; objectUrl: string; zoom: number; fit: boolean; onFit: () => void; onZoom: (zoom: number) => void }) {
  return <div className="fox-preview-surface"><PreviewToolbar><span className="fox-preview-toolbar-label">{fit ? '适应窗口' : `${Math.round(zoom * 100)}%`}</span><PreviewIconButton label="缩小" disabled={!fit && zoom <= MIN_IMAGE_ZOOM} onClick={() => onZoom(zoom - .25)}><Minus /></PreviewIconButton><PreviewIconButton label="放大" disabled={!fit && zoom >= MAX_IMAGE_ZOOM} onClick={() => onZoom(zoom + .25)}><Plus /></PreviewIconButton><PreviewIconButton label="适应窗口" active={fit} onClick={onFit}><Maximize2 /></PreviewIconButton><PreviewIconButton label="实际大小" active={!fit && zoom === 1} onClick={() => onZoom(1)}><RotateCcw /></PreviewIconButton></PreviewToolbar><div className={`fox-image-preview ${fit ? 'is-fit' : ''}`}><img alt={filename} src={objectUrl} style={fit ? undefined : { transform: `scale(${zoom})` }} /></div></div>
}

function TextPreview({ text, sourceLocator }: { text: string; sourceLocator?: KnowledgeSourceLocator | null }) {
  const locatorQuery = sourceLocatorQuery(sourceLocator)
  const [query, setQuery] = useState(locatorQuery)
  const [activeIndex, setActiveIndex] = useState(0)
  const [wrap, setWrap] = useState(true)
  const deferredQuery = useDeferredValue(query.trim())
  const activeMatch = useRef<HTMLElement | null>(null)
  const matches = useMemo(() => findTextMatches(text, deferredQuery), [deferredQuery, text])
  useEffect(() => setQuery(locatorQuery), [locatorQuery])
  useEffect(() => { setActiveIndex(0) }, [deferredQuery])
  useEffect(() => { activeMatch.current?.scrollIntoView({ block: 'center', behavior: 'smooth' }) }, [activeIndex])
  const move = (offset: number) => {
    if (!matches.indices.length) return
    setActiveIndex((current) => (current + offset + matches.indices.length) % matches.indices.length)
  }
  return <div className="fox-preview-surface"><PreviewToolbar><label className="fox-preview-search"><Search /><Input aria-label="在文档中搜索" placeholder="搜索文本" value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') move(event.shiftKey ? -1 : 1) }} />{query && <button type="button" aria-label="清除搜索" onClick={() => setQuery('')}><X /></button>}</label><span className="fox-preview-toolbar-label">{deferredQuery ? matches.truncated ? `${matches.indices.length}+ 处` : `${matches.indices.length} 处` : '文本预览'}</span><PreviewIconButton label="上一个匹配" disabled={!matches.indices.length} onClick={() => move(-1)}><ChevronUp /></PreviewIconButton><PreviewIconButton label="下一个匹配" disabled={!matches.indices.length} onClick={() => move(1)}><ChevronDown /></PreviewIconButton><PreviewIconButton label={wrap ? '关闭自动换行' : '开启自动换行'} active={wrap} onClick={() => setWrap((value) => !value)}><WrapText /></PreviewIconButton></PreviewToolbar><pre className={`fox-text-document-preview ${wrap ? 'is-wrapped' : ''}`}>{renderHighlightedText(text, deferredQuery, matches.indices, activeIndex, activeMatch)}</pre></div>
}

function sourceLocatorQuery(locator?: KnowledgeSourceLocator | null) {
  const value = locator?.anchor || locator?.excerpt || ''
  const line = value.split(/\r?\n/).map((item) => item.trim()).find((item) => item.length >= 4) ?? value.trim()
  return line.length > 80 ? line.slice(0, 80).trim() : line
}

function SourceLocatorNotice({ locator }: { locator: KnowledgeSourceLocator }) {
  const details = [
    locator.page ? `第 ${locator.page} 页` : '',
    locator.anchor || '',
  ].filter(Boolean).join(' · ')
  if (!details && !locator.excerpt) return null
  return <div className="fox-file-preview-notice fox-source-locator-notice"><Search /><span><b>已定位到引用来源</b><small>{details || '正在查找引用片段'}</small></span></div>
}

function findTextMatches(text: string, query: string) {
  if (!query) return { indices: [] as number[], truncated: false }
  const indices: number[] = []
  const source = text.toLowerCase()
  const target = query.toLowerCase()
  let cursor = 0
  while (cursor <= source.length - target.length) {
    const index = source.indexOf(target, cursor)
    if (index < 0) break
    indices.push(index)
    cursor = index + Math.max(1, target.length)
    if (indices.length >= TEXT_SEARCH_RESULT_LIMIT) return { indices, truncated: source.indexOf(target, cursor) >= 0 }
  }
  return { indices, truncated: false }
}

function renderHighlightedText(text: string, query: string, indices: number[], activeIndex: number, activeMatch: { current: HTMLElement | null }) {
  if (!query || !indices.length) return text
  const output: ReactNode[] = []
  let cursor = 0
  indices.forEach((index, matchIndex) => {
    output.push(text.slice(cursor, index))
    output.push(<mark key={`${index}:${matchIndex}`} className={matchIndex === activeIndex ? 'is-active' : ''} ref={(node) => { if (matchIndex === activeIndex) activeMatch.current = node }}>{text.slice(index, index + query.length)}</mark>)
    cursor = index + query.length
  })
  output.push(text.slice(cursor))
  return output
}

function PreviewModuleLoading({ label }: { label: string }) {
  return <div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>{label}</b></div>
}

function DelimitedPreview({ rows, overflow }: { rows: string[][]; overflow: boolean }) {
  if (!rows.length) return <div className="fox-file-preview-state"><FileText /><b>表格为空</b></div>
  const width = Math.max(...rows.map((row) => row.length))
  const dataRows = rows.slice(1)
  const [scrollTop, setScrollTop] = useState(0)
  const [viewportHeight, setViewportHeight] = useState(480)
  const viewport = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    if (!viewport.current || typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(([entry]) => setViewportHeight(entry.contentRect.height))
    observer.observe(viewport.current)
    return () => observer.disconnect()
  }, [])
  const window = virtualRowWindow(dataRows.length, scrollTop, viewportHeight)
  const { start, end } = window
  const visibleRows = dataRows.slice(start, end)
  return <div className="fox-preview-surface"><PreviewToolbar><span className="fox-preview-toolbar-label">{dataRows.length} 行 · {width} 列</span><span className="fox-preview-toolbar-hint">只读数据预览</span></PreviewToolbar><div ref={viewport} className="fox-delimited-preview" onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}><table><thead><tr>{Array.from({ length: width }, (_, index) => <th key={index}>{rows[0]?.[index] || `列 ${index + 1}`}</th>)}</tr></thead><tbody>{window.beforeHeight > 0 && <tr className="fox-table-spacer" aria-hidden="true"><td colSpan={width} style={{ height: window.beforeHeight }} /></tr>}{visibleRows.map((row, rowIndex) => <tr key={start + rowIndex}>{Array.from({ length: width }, (_, columnIndex) => <td key={columnIndex}>{row[columnIndex] ?? ''}</td>)}</tr>)}{window.afterHeight > 0 && <tr className="fox-table-spacer" aria-hidden="true"><td colSpan={width} style={{ height: window.afterHeight }} /></tr>}</tbody></table>{overflow && <p>仅展示前 500 行或 100 列，请下载原文件查看全部内容。</p>}</div></div>
}

function PresentationPreview({ markdown, emptyMessage }: { markdown: string; emptyMessage?: string | null }) {
  const slides = useMemo(() => parsePresentationSlides(markdown), [markdown])
  const [active, setActive] = useState(0)
  useEffect(() => setActive(0), [markdown])
  const slide = slides[Math.min(active, Math.max(0, slides.length - 1))]
  if (!slide) return <div className="fox-file-preview-state"><FileText /><b>演示文稿没有可显示的解析内容</b><p>{emptyMessage}</p></div>
  return <div className="fox-presentation-preview"><nav aria-label="演示文稿页面">{slides.map((item, index) => <button type="button" key={`${index}:${item.title}`} className={index === active ? 'is-active' : ''} onClick={() => setActive(index)}><span>{index + 1}</span><b title={item.title}>{item.title}</b></button>)}</nav><article><header><span>第 {active + 1} / {slides.length} 页</span><b>{slide.title}</b></header><MessageResponse className="fox-rich-document-preview">{slide.content}</MessageResponse></article></div>
}
