import { NewConversationMascot } from '../components/NewConversationMascot'
import { ConversationEmptyState } from '@/components/ai-elements/conversation'
import { type ArtifactRecord, type ConversationMessage } from '@/features/conversations/model/types'
import { useEffect, useMemo, useRef, useState } from 'react'
import { splitAssistantContent } from './fox-assistant-content'

export function ConversationEmptyPrompt(_props: { suggestions?: string[]; onStart: (suggestion: string) => void }) {
  return <ConversationEmptyState className="fox-empty-conversation">
    <div className="fox-empty-heading"><NewConversationMascot /><div className="fox-empty-heading-copy"><h2>今天想做什么？</h2><p>Fox 可以和你对话、处理本机文件，也可以从资料库中检索资料。</p></div></div>
  </ConversationEmptyState>
}

function turnTarget(scope: Element, messageId: string): HTMLElement | null {
  const anchor = scope.querySelector<HTMLElement>(`[data-turn-message-id="${CSS.escape(messageId)}"]`)
  if (!anchor) return null
  // display: contents anchors have no scrollable box; use their rendered message.
  return getComputedStyle(anchor).display === 'contents' ? anchor.firstElementChild as HTMLElement | null : anchor
}

function turnViewport(target: HTMLElement): HTMLElement | null {
  for (let parent = target.parentElement; parent; parent = parent.parentElement) {
    if (/(auto|scroll)/.test(getComputedStyle(parent).overflowY) && parent.scrollHeight > parent.clientHeight) return parent
  }
  return null
}

export type TurnSpan = { id: string; top: number; bottom: number };

/**
 * SPEC item 8：回合导航的选中逻辑。滚动到底时强制选中最后一轮；否则选中「整轮
 * （用户消息 + 该轮全部助手消息）范围覆盖视口中线」的那一轮；没有覆盖中线的
 * （停在两轮之间的空隙）取距离中线最近的一轮，距离相同取靠前的一轮。
 * 不再用「最接近视口顶部的用户消息」当锚点——那样长回复会把选中项钉在屏外。
 */
export function computeActiveTurnId(spans: TurnSpan[], viewport: { top: number; bottom: number }, atBottom: boolean): string | null {
  if (!spans.length) return null
  if (atBottom) return spans.at(-1)!.id
  const mid = (viewport.top + viewport.bottom) / 2
  let best: string | null = null
  let bestDistance = Infinity
  for (const span of spans) {
    const distance = mid < span.top ? span.top - mid : mid > span.bottom ? mid - span.bottom : 0
    if (distance < bestDistance) {
      bestDistance = distance
      best = span.id
    }
  }
  return best
}

export function ConversationTurnRail({ messages, artifacts }: { messages: ConversationMessage[]; artifacts: ArtifactRecord[] }) {
  const [hoveredId, setHoveredId] = useState<string | null>(null)
  const [activeId, setActiveId] = useState<string | null>(null)
  const railRef = useRef<HTMLElement | null>(null)
  const turns = useMemo(() => messages.reduce<Array<{ user: ConversationMessage; assistants: ConversationMessage[]; assistant?: ConversationMessage; preview?: string; artifacts: ArtifactRecord[]; url?: string }>>((items, message) => {
    if (message.role === 'user') {
      items.push({ user: message, assistants: [], artifacts: [] })
      return items
    }
    if (message.role === 'assistant' && items.length) {
      const current = items.at(-1)!
      current.assistants.push(message)
    }
    return items
  }, []).map((turn) => {
    const assistant = turn.assistants.at(-1)
    const answer = [...turn.assistants].reverse().map(message => splitAssistantContent(message.content).answer.trim()).find(Boolean)
    const runIds = new Set(turn.assistants.map(message => message.runId).filter(Boolean))
    const files = artifacts.filter(artifact => runIds.has(artifact.runId))
    const preview = answer || (files.length ? `已生成 ${files.length} 个文件` : assistant?.status === 'streaming' || assistant?.status === 'running' ? 'Fox 正在回复…' : '暂无文字回复')
    const content = `${turn.user.content}\n${answer ?? ''}`
    return {
      ...turn,
      assistant, preview,
      artifacts: files,
      url: content.match(/https?:\/\/[^\s)]+/)?.[0],
    }
  }), [artifacts, messages])

  useEffect(() => {
    if (!turns.length) {
      setActiveId(null)
      return
    }
    // 新一轮开始时先落在最后一轮，随后的量测再按视口中线校正。
    setActiveId((current) => current && turns.some((turn) => turn.user.id === current) ? current : turns.at(-1)!.user.id)
    const scope = railRef.current?.closest('.fox-thread, .fox-conversation')
    if (!scope) return
    let viewportRoot: HTMLElement | null = null
    for (const turn of turns) {
      const target = turnTarget(scope, turn.user.id)
      if (target) {
        viewportRoot = turnViewport(target)
        break
      }
    }
    if (!viewportRoot) return
    const root = viewportRoot
    let frame = 0
    const update = () => {
      frame = 0
      const spans: TurnSpan[] = []
      for (const turn of turns) {
        let top = Infinity
        let bottom = -Infinity
        for (const id of [turn.user.id, ...turn.assistants.map((assistant) => assistant.id)]) {
          const target = turnTarget(scope, id)
          if (!target) continue
          const rect = target.getBoundingClientRect()
          top = Math.min(top, rect.top)
          bottom = Math.max(bottom, rect.bottom)
        }
        if (top !== Infinity) spans.push({ id: turn.user.id, top, bottom })
      }
      if (!spans.length) return
      const viewport = root.getBoundingClientRect()
      const atBottom = root.scrollHeight - root.scrollTop - root.clientHeight <= 8
      const next = computeActiveTurnId(spans, { top: viewport.top, bottom: viewport.bottom }, atBottom)
      if (next) setActiveId((current) => current === next ? current : next)
    }
    const schedule = () => {
      if (!frame) frame = window.requestAnimationFrame(update)
    }
    schedule()
    root.addEventListener('scroll', schedule, { passive: true })
    window.addEventListener('resize', schedule)
    // 流式回复在消息数组身份不变时也会撑高内容，用 ResizeObserver 跟踪。
    const resizeObserver = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(schedule) : null
    if (resizeObserver) for (const child of Array.from(root.children)) resizeObserver.observe(child)
    return () => {
      if (frame) window.cancelAnimationFrame(frame)
      root.removeEventListener('scroll', schedule)
      window.removeEventListener('resize', schedule)
      resizeObserver?.disconnect()
    }
  }, [turns])

  // 只有一轮时左侧短横线会像误放的字符，至少两轮才显示回合导航。
  if (turns.length < 2) return null
  const hovered = turns.find((turn) => turn.user.id === hoveredId)
  const scrollToTurn = (messageId: string) => {
    setActiveId(messageId)
    const scope = railRef.current?.closest('.fox-thread, .fox-conversation')
    const target = scope ? turnTarget(scope, messageId) : null
    if (!target) return
    const viewport = turnViewport(target)
    setHoveredId(null)
    const behavior = window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'instant' : 'smooth'
    if (viewport) {
      viewport.scrollTo({ top: viewport.scrollTop + target.getBoundingClientRect().top - viewport.getBoundingClientRect().top - 12, behavior })
    } else target.scrollIntoView({ behavior, block: 'start' })
  }

  return <nav ref={railRef} className="fox-conversation-rail" aria-label="当前对话历史导航" onMouseLeave={() => setHoveredId(null)}>
    <div className="fox-conversation-marks">{turns.map((turn, index) => <button type="button" key={turn.user.id} className={turn.user.id === activeId ? 'is-active' : ''} aria-label={`第 ${index + 1} 轮：${turn.user.content.slice(0, 40)}`} onMouseEnter={() => setHoveredId(turn.user.id)} onFocus={() => setHoveredId(turn.user.id)} onBlur={() => setHoveredId(null)} onClick={() => scrollToTurn(turn.user.id)}><span /></button>)}</div>
    <div className={`fox-conversation-preview ${hovered ? 'is-visible' : ''}`}>
      <strong>{hovered?.user.content || '对话回合'}</strong>
      <p>{hovered?.preview?.slice(0, 360) || ''}</p>
      {hovered && (hovered.artifacts.length > 0 || hovered.url) && <div className="fox-conversation-preview-files">
        {hovered.artifacts.slice(0, 2).map(artifact => <span key={artifact.id}>{artifact.displayName}</span>)}
        {hovered.artifacts.length > 2 && <span>+{hovered.artifacts.length - 2}</span>}
        {!hovered.artifacts.length && hovered.url && <span>{hovered.url.replace(/^https?:\/\//, '').split('/')[0]}</span>}
      </div>}
    </div>
  </nav>
}
