import { useCallback, useEffect, useRef, useState } from 'react'
import { CalendarDays, ChevronDown, ChevronUp, LoaderCircle, Search, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { notify as toast } from '@/features/notifications'
import type { ContactBubbleSearchHit, ConversationDetail } from '@/features/conversations/model/types'
import { formatClockTime } from '@/lib/time-format'
import type { Companion } from './api'
import { ContactAvatar } from './chat-controls'
import type { ContactLocateRequest } from './context'

export type ContactSearchMode = 'closed' | 'panel' | 'locating' | 'chip'
export type ContactSearchRange = 'all' | '7d' | '30d' | 'custom'

export interface ContactChatSearch {
  mode: ContactSearchMode
  draft: string
  setDraft: (value: string) => void
  query: string
  results: ContactBubbleSearchHit[]
  total: number
  selected: number
  setSelected: (index: number) => void
  moveSelection: (delta: number) => void
  range: ContactSearchRange
  setRange: (range: ContactSearchRange) => void
  customSince: string
  setCustomSince: (value: string) => void
  customUntil: string
  setCustomUntil: (value: string) => void
  activeIndex: number
  busy: boolean
  error: string | null
  open: () => void
  close: () => void
  locate: (index: number) => void
  step: (delta: number) => void
}

/** 「今天 14:33」 / 「10 月 12 日」 / 「2025 年 10 月 12 日」, local timezone. */
export function formatSearchTime(value: number, now = Date.now()): string {
  const date = new Date(value)
  const current = new Date(now)
  if (date.toDateString() === current.toDateString()) return `今天 ${formatClockTime(value)}`
  const monthDay = `${date.getMonth() + 1} 月 ${date.getDate()} 日`
  return date.getFullYear() === current.getFullYear() ? monthDay : `${date.getFullYear()} 年 ${monthDay}`
}

function rangeBounds(range: ContactSearchRange, customSince: string, customUntil: string, now = Date.now()): { since?: number; until?: number } {
  if (range === '7d') return { since: now - 7 * 86_400_000 }
  if (range === '30d') return { since: now - 30 * 86_400_000 }
  if (range !== 'custom') return {}
  const since = customSince ? new Date(`${customSince}T00:00:00`).getTime() : Number.NaN
  const until = customUntil ? new Date(`${customUntil}T23:59:59.999`).getTime() : Number.NaN
  return { since: Number.isNaN(since) ? undefined : since, until: Number.isNaN(until) ? undefined : until }
}

function findBubbleElement(bubbleId: string): HTMLElement | null {
  const value = bubbleId.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
  return document.querySelector<HTMLElement>(`[data-contact-message-id="${value}"], [data-turn-message-id="${value}"]`)
}

let highlightedElement: HTMLElement | null = null

export function clearContactSearchHighlight() {
  highlightedElement?.classList.remove('is-search-hit')
  highlightedElement = null
  if (typeof CSS !== 'undefined' && 'highlights' in CSS) CSS.highlights.delete('fox-contact-search')
}

/** Ring the located bubble and mark query occurrences. Text marks use the CSS
 * Custom Highlight API so React-owned DOM is never mutated; the ring survives
 * re-renders because K1 keeps the bubble's identity (and React key) stable. */
function highlightBubble(element: HTMLElement, query: string) {
  clearContactSearchHighlight()
  highlightedElement = element
  element.classList.add('is-search-hit')
  // Turn anchors use display: contents, so scroll their rendered message instead.
  const scrollTarget = element.classList.contains('fox-turn-anchor') ? element.firstElementChild ?? element : element
  scrollTarget.scrollIntoView({ behavior: 'smooth', block: 'center' })
  if (!query || typeof CSS === 'undefined' || !('highlights' in CSS) || typeof Highlight === 'undefined') return
  // ASCII-only folding keeps offsets aligned with the original text and matches
  // the SQLite LIKE semantics used by the backend.
  const fold = (text: string) => text.replace(/[A-Z]/g, char => char.toLowerCase())
  const needle = fold(query)
  const ranges: Range[] = []
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT)
  while (walker.nextNode()) {
    const node = walker.currentNode
    const text = fold(node.textContent ?? '')
    for (let index = text.indexOf(needle); index !== -1; index = text.indexOf(needle, index + needle.length)) {
      const range = new Range()
      range.setStart(node, index)
      range.setEnd(node, index + needle.length)
      ranges.push(range)
    }
  }
  if (ranges.length) CSS.highlights.set('fox-contact-search', new Highlight(...ranges))
}

export function useContactChatSearch({ conversationId, detail, loadEarlierMessages, locateRequest }: {
  conversationId: string | undefined
  detail: ConversationDetail | null
  loadEarlierMessages: () => Promise<void>
  locateRequest?: ContactLocateRequest | null
}): ContactChatSearch {
  const [mode, setMode] = useState<ContactSearchMode>('closed')
  const [draft, setDraft] = useState('')
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<ContactBubbleSearchHit[]>([])
  const [total, setTotal] = useState(0)
  const [selected, setSelected] = useState(0)
  const [range, setRange] = useState<ContactSearchRange>('all')
  const [customSince, setCustomSince] = useState('')
  const [customUntil, setCustomUntil] = useState('')
  const [activeIndex, setActiveIndex] = useState(0)
  const [target, setTarget] = useState<ContactBubbleSearchHit | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const loaderRef = useRef(loadEarlierMessages)
  loaderRef.current = loadEarlierMessages
  // C05 external locate (profile file list): ring the bubble, then return the
  // topbar to normal instead of showing the search chip.
  const externalRef = useRef(false)

  const close = useCallback(() => {
    clearContactSearchHighlight()
    externalRef.current = false
    setMode('closed')
    setTarget(null)
    setDraft('')
    setQuery('')
    setResults([])
    setTotal(0)
    setSelected(0)
    setActiveIndex(0)
    setRange('all')
    setCustomSince('')
    setCustomUntil('')
    setBusy(false)
    setError(null)
  }, [])

  // Switching contacts must not carry the previous contact's results or hit.
  useEffect(() => { close() }, [conversationId, close])

  useEffect(() => {
    if (!locateRequest || !conversationId) return
    externalRef.current = true
    setTarget({ id: locateRequest.id } as ContactBubbleSearchHit)
    setQuery('')
    setError(null)
    setMode('locating')
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [locateRequest?.key, conversationId])

  useEffect(() => {
    if (mode !== 'panel') return
    const trimmed = draft.trim()
    if (!trimmed || !conversationId) {
      setResults([])
      setTotal(0)
      setBusy(false)
      return
    }
    setBusy(true)
    let cancelled = false
    const handle = setTimeout(() => {
      desktopClient.searchContactChat(conversationId, trimmed, { ...rangeBounds(range, customSince, customUntil), limit: 100 })
        .then(page => {
          if (cancelled) return
          setResults(page.results)
          setTotal(page.total)
          setSelected(0)
          setQuery(trimmed)
          setBusy(false)
          setError(null)
        })
        .catch(cause => {
          if (cancelled) return
          setBusy(false)
          setError(cause instanceof Error ? cause.message : String(cause))
        })
    }, 250)
    return () => { cancelled = true; clearTimeout(handle) }
  }, [mode, draft, range, customSince, customUntil, conversationId])

  // Page history back until the target bubble renders, then ring it. The
  // loader guards concurrent calls itself; each merged page re-runs this effect.
  useEffect(() => {
    if (mode !== 'locating' || !target || !conversationId) return
    const settle = (element: HTMLElement) => {
      highlightBubble(element, query)
      if (externalRef.current) {
        externalRef.current = false
        setMode('closed')
      } else {
        setMode('chip')
      }
    }
    const element = findBubbleElement(target.id)
    if (element) {
      settle(element)
      return
    }
    if (detail?.conversation?.id === conversationId && detail.hasEarlierMessages) {
      void loaderRef.current()
      return
    }
    let cancelled = false
    let attempts = 0
    const retry = () => {
      if (cancelled) return
      const found = findBubbleElement(target.id)
      if (found) {
        settle(found)
        return
      }
      if (++attempts < 30) requestAnimationFrame(retry)
      else if (externalRef.current) {
        externalRef.current = false
        toast.error('未能在历史中找到这条消息')
        setMode('closed')
      } else {
        setError('未能在历史中找到这条消息')
        setMode('panel')
      }
    }
    requestAnimationFrame(retry)
    return () => { cancelled = true }
  }, [mode, target, detail, conversationId, query])

  const open = useCallback(() => { setMode('panel'); setError(null) }, [])
  const locate = useCallback((index: number) => {
    const hit = results[index]
    if (!hit) return
    setActiveIndex(index)
    setTarget(hit)
    setError(null)
    setMode('locating')
  }, [results])
  const step = useCallback((delta: number) => {
    if (results.length) locate((activeIndex + delta + results.length) % results.length)
  }, [results.length, activeIndex, locate])
  const moveSelection = useCallback((delta: number) => {
    setSelected(previous => results.length ? (previous + delta + results.length) % results.length : 0)
  }, [results.length])

  return {
    mode, draft, setDraft, query, results, total, selected, setSelected, moveSelection,
    range, setRange, customSince, setCustomSince, customUntil, setCustomUntil,
    activeIndex, busy, error, open, close, locate, step,
  }
}

export function ContactSearchBar({ search, onClose }: { search: ContactChatSearch; onClose: () => void }) {
  const inputRef = useRef<HTMLInputElement>(null)
  useEffect(() => { inputRef.current?.focus({ preventScroll: true }) }, [])
  return <div className="fox-contact-search-bar">
    <Search size={14} aria-hidden="true" />
    <input
      ref={inputRef}
      value={search.draft}
      placeholder="搜索私聊"
      aria-label="搜索私聊"
      onChange={event => search.setDraft(event.target.value)}
      onKeyDown={event => {
        if (event.nativeEvent.isComposing) return
        if (event.key === 'ArrowDown') { event.preventDefault(); search.moveSelection(1) }
        else if (event.key === 'ArrowUp') { event.preventDefault(); search.moveSelection(-1) }
        else if (event.key === 'Enter') { event.preventDefault(); search.locate(search.selected) }
      }}
    />
    {search.busy && <LoaderCircle size={14} className="is-spinning" aria-label="搜索中" />}
    <Button type="button" variant="ghost" size="icon-xs" aria-label="关闭搜索" onClick={onClose}><X /></Button>
  </div>
}

export function ContactSearchChip({ search, onClose }: { search: ContactChatSearch; onClose: () => void }) {
  return <div className="fox-contact-search-chip" role="status" aria-label="搜索定位">
    <Search size={13} aria-hidden="true" />
    <b>{search.query || '定位消息'}</b>
    <span>{search.results.length ? `${search.activeIndex + 1} / ${search.results.length}` : search.query ? '0 / 0' : '正在定位…'}</span>
    <Button type="button" variant="ghost" size="icon-xs" aria-label="上一条结果" disabled={!search.results.length} onClick={() => search.step(-1)}><ChevronUp /></Button>
    <Button type="button" variant="ghost" size="icon-xs" aria-label="下一条结果" disabled={!search.results.length} onClick={() => search.step(1)}><ChevronDown /></Button>
    <Button type="button" variant="ghost" size="icon-xs" aria-label="关闭搜索" onClick={onClose}><X /></Button>
  </div>
}

const RANGE_LABELS: [ContactSearchRange, string][] = [['all', '时间不限'], ['7d', '最近 7 天'], ['30d', '最近 30 天'], ['custom', '自定义']]

export function ContactSearchPanel({ search, contact, contactName }: {
  search: ContactChatSearch
  contact: Companion | null | undefined
  contactName: string
}) {
  return <div className="fox-contact-search-panel" role="dialog" aria-label="搜索私聊结果">
    <div className="fox-contact-search-head">
      <div className="fox-contact-search-caption">
        <strong>{search.total} 条消息</strong>
        <span>只搜索和{contactName}的私聊</span>
      </div>
      <Select value={search.range} onValueChange={value => search.setRange(value as ContactSearchRange)}>
        <SelectTrigger size="sm" className="fox-contact-search-range-trigger" aria-label="时间范围">
          <CalendarDays aria-hidden="true" /><SelectValue />
        </SelectTrigger>
        <SelectContent align="end" className="fox-contact-search-range-menu">
          {RANGE_LABELS.map(([value, label]) => <SelectItem key={value} value={value}>{label}</SelectItem>)}
        </SelectContent>
      </Select>
    </div>
    {search.range === 'custom' && <div className="fox-contact-search-custom">
      <label><span>开始</span><Input type="date" value={search.customSince} aria-label="开始日期" onChange={event => search.setCustomSince(event.target.value)} /></label>
      <label><span>结束</span><Input type="date" value={search.customUntil} aria-label="结束日期" onChange={event => search.setCustomUntil(event.target.value)} /></label>
    </div>}
    <div className="fox-contact-search-results">
      {search.error
        ? <p className="fox-contact-search-empty" role="alert">{search.error}</p>
        : !search.draft.trim()
          ? <p className="fox-contact-search-empty">输入关键词，搜索和{contactName}的私聊</p>
          : !search.busy && search.results.length === 0
            ? <p className="fox-contact-search-empty">没有找到相关消息</p>
            : search.results.map((hit, index) => {
              // Offsets are Unicode char counts (Rust chars()); slice by code
              // point so emoji in the snippet cannot shift the highlight.
              const chars = Array.from(hit.snippet)
              return <button
                type="button"
                key={hit.id}
                className={`fox-contact-search-result${index === search.selected ? ' is-selected' : ''}`}
                onClick={() => search.locate(index)}
                onMouseEnter={() => search.setSelected(index)}
              >
                {hit.author === 'user'
                  ? <span className="fox-contact-search-me" aria-hidden="true">我</span>
                  : contact ? <ContactAvatar contact={contact} /> : <span className="fox-contact-search-me" aria-hidden="true">{[...contactName][0]}</span>}
                <span className="fox-contact-search-body">
                  <b>{hit.author === 'user' ? '我' : contactName}</b>
                  <p>{chars.slice(0, hit.matchStart).join('')}<mark>{chars.slice(hit.matchStart, hit.matchEnd).join('')}</mark>{chars.slice(hit.matchEnd).join('')}</p>
                </span>
                <time dateTime={new Date(hit.deliveredAt).toISOString()}>{formatSearchTime(hit.deliveredAt)}</time>
              </button>
            })}
    </div>
    <div className="fox-contact-search-foot">
      <span><kbd>↑</kbd> <kbd>↓</kbd> 选择</span>
      <span><kbd>Enter</kbd> 跳到这条消息</span>
      <span><kbd>Esc</kbd> 关闭</span>
    </div>
  </div>
}
