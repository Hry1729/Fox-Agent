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
    setActiveId((current) => current && turns.some((turn) => turn.user.id === current) ? current : turns.at(-1)!.user.id)
    const scope = railRef.current?.closest('.fox-thread, .fox-conversation')
    if (!scope) return
    const anchors = turns.flatMap((turn) => {
      const target = turnTarget(scope, turn.user.id)
      return target ? [{ target, id: turn.user.id }] : []
    })
    if (!anchors.length) return
    const root = turnViewport(anchors[0].target)
    const ids = new Map(anchors.map(anchor => [anchor.target, anchor.id]))
    const visible = new Map<string, number>()
    const observer = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        const id = ids.get(entry.target as HTMLElement)
        if (!id) return
        if (entry.isIntersecting) visible.set(id, entry.intersectionRatio)
        else visible.delete(id)
      })
      const top = (root?.getBoundingClientRect().top ?? 0) + 12
      const positions = new Map(anchors.map(({ id, target }) => [id, Math.abs(target.getBoundingClientRect().top - top)]))
      const current = [...visible.keys()].sort((left, right) => (positions.get(left) ?? Infinity) - (positions.get(right) ?? Infinity))[0]
      if (current) setActiveId(current)
    }, { root, threshold: [0.05, 0.25, 0.5, 0.8] })
    anchors.forEach(({ target }) => observer.observe(target))
    return () => observer.disconnect()
  }, [turns])

  if (!turns.length) return null
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
