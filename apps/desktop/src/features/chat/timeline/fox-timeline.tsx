import { type ExpertBindingView } from '../components/ExpertBindingChip'
import { FoxAssistantAvatar, RunStatusText } from '../components/FoxAssistantAvatar'
import { NewConversationMascot } from '../components/NewConversationMascot'
import { UserMessageBubble } from '../components/UserMessageBubble'
import { type GoalProgressData } from '../hooks/use-goal-progress'
import { citations } from '../mock-data'
import { Conversation, ConversationContent, ConversationEmptyState, ConversationViewportState } from '@/components/ai-elements/conversation'
import { Message, MessageAction, MessageActions, MessageContent } from '@/components/ai-elements/message'
import { Source, Sources, SourcesContent, SourcesTrigger } from '@/components/ai-elements/sources'
import { Suggestion, Suggestions } from '@/components/ai-elements/suggestion'
import { type AgentRecord, type ArtifactRecord, type AttachmentRecord, type ConversationMessage, type DesktopErrorDetails, type RunEventRecord, type RunRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { ChevronDown, ChevronRight, Copy, FileEdit, FileText, Globe2, ImagePlus, MessageCircleMore, Sparkles } from 'lucide-react'
import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { AssistantProcess, DeniedPrompt, ErrorPrompt, LicenseInlineCitation, licenseAnswerMarkdown } from '../demo/fox-demo-prompts'
import { type ChatState, MarkdownResponse } from '../fox-shared'
import { type RuntimeSource } from './fox-runtime-process-model'
import { ConversationBottomDock, RuntimeTimeline, splitAssistantContent } from './fox-runtime-timeline'

/**
 * The classic new-conversation empty state: mascot heading plus the expert's
 * opening suggestions (or the default four). Clicking a suggestion fills the
 * composer draft via `onStart`. Shared by the legacy timeline and the
 * assistant-ui thread's empty state so both engines show the same screen.
 */
export function ConversationEmptyPrompt({ suggestions, onStart }: { suggestions?: string[]; onStart: (suggestion: string) => void }) {
  const starterSuggestions = suggestions?.map((item) => item.trim()).filter(Boolean).slice(0, 6)
  const visibleSuggestions = starterSuggestions?.length ? starterSuggestions : [
    '整理这个项目中的文档',
    '从知识库查找相关资料',
    '读取一个文件并帮我分析',
    '帮我规划并完成一个任务',
  ]
  return (
    <ConversationEmptyState className="fox-empty-conversation">
      <div className="fox-empty-heading">
        <NewConversationMascot />
        <div className="fox-empty-heading-copy"><h2>今天想一起做点什么？</h2><p>Fox 可以和你对话、处理本地文件，也可以从知识库中检索资料。</p></div>
      </div>
      <Suggestions className="fox-starter-suggestions">
        {visibleSuggestions.map((suggestion) => <Suggestion key={suggestion} suggestion={suggestion} onClick={onStart} />)}
      </Suggestions>
    </ConversationEmptyState>
  )
}

function Timeline({ empty, state, prompt, openingSuggestions, runtimeMessages, runtimeAttachments, runtimeArtifacts, expertBindings, agents, pendingMessage, runtimeEvents, runtimeRuns, runtimeRunId, runtimeRunModel, runtimeRunning = false, runtimeReply, runtimeError, runtimeErrorDetails, assistantName, planRevisions, hasEarlierMessages, loadingEarlierMessages, onLoadEarlierMessages, onApprove, onDeny, onRetry, onRerun, onFork, onAnswer, onStart, onOpenSource, onOpenArtifact, onViewExpert, onOpenFileInSidebar, onRevealFileInExplorer }: { empty: boolean; state: ChatState; prompt: string; openingSuggestions?: string[]; runtimeMessages?: ConversationMessage[]; runtimeAttachments?: AttachmentRecord[]; runtimeArtifacts?: ArtifactRecord[]; expertBindings?: ExpertBindingView[]; agents?: AgentRecord[]; pendingMessage?: ConversationMessage | null; runtimeEvents?: RunEventRecord[]; runtimeRuns?: RunRecord[]; runtimeRunId?: string; runtimeRunModel?: string; runtimeRunning?: boolean; runtimeReply?: string; runtimeError?: string | null; runtimeErrorDetails?: DesktopErrorDetails | null; assistantName?: string; planRevisions?: GoalProgressData['planRevisions']; hasEarlierMessages?: boolean; loadingEarlierMessages?: boolean; onLoadEarlierMessages?: () => void; onApprove: () => void; onDeny: () => void; onRetry: () => void; onRerun: (messageId: string, prompt: string) => Promise<boolean>; onFork?: (messageId: string) => void; onAnswer: (answer: string) => void; onStart: (suggestion: string) => void; onOpenSource?: (source: RuntimeSource) => void; onOpenArtifact?: (artifact: ArtifactRecord) => void; onViewExpert?: (expertId: string) => void; onOpenFileInSidebar?: (path: string) => void; onRevealFileInExplorer?: (path: string) => void }) {
  const running = state === 'running'
  const latestPlanRevision = planRevisions?.reduce((latest, plan) => !latest || plan.revision > latest.revision ? plan : latest, undefined as GoalProgressData['planRevisions'][number] | undefined)

  if (empty) {
    return (
        <Conversation className="fox-conversation">
        <ConversationViewportState />
        <ConversationEmptyPrompt suggestions={openingSuggestions} onStart={onStart} />
      </Conversation>
    )
  }
  if (runtimeMessages) {
    return <RuntimeTimeline messages={runtimeMessages} attachments={runtimeAttachments ?? []} artifacts={runtimeArtifacts ?? []} expertBindings={expertBindings} agents={agents} pendingMessage={pendingMessage} events={runtimeEvents ?? []} runs={runtimeRuns} activeRunId={runtimeRunId} activeRunModel={runtimeRunModel} runtimeRunning={runtimeRunning} state={state} streamingText={runtimeReply ?? ''} runtimeError={runtimeError} runtimeErrorDetails={runtimeErrorDetails} assistantName={assistantName} planRevision={latestPlanRevision} hasEarlierMessages={hasEarlierMessages} loadingEarlierMessages={loadingEarlierMessages} onLoadEarlierMessages={onLoadEarlierMessages} onRetry={onRetry} onRerun={onRerun} onFork={onFork} onOpenSource={onOpenSource} onOpenArtifact={onOpenArtifact} onViewExpert={onViewExpert} onOpenFileInSidebar={onOpenFileInSidebar} onRevealFileInExplorer={onRevealFileInExplorer} />
  }
  return (
    <Conversation className="fox-conversation">
      <ConversationViewportState />
      <ConversationContent className="fox-conversation-content" scrollClassName="fox-conversation-scroll">
        <div className="fox-thread-date"><span>今天</span></div>
        <div className="fox-turn-anchor">
        <Message from="user" className="fox-message fox-user-message">
          <UserMessageBubble>{prompt}</UserMessageBubble>
          <div className="fox-user-meta"><span>DeepSeek V3.2 · 刚刚</span><span><MessageAction tooltip="复制"><Copy size={13} /></MessageAction><MessageAction tooltip="编辑"><FileEdit size={13} /></MessageAction></span></div>
        </Message>
        </div>
        <div className="fox-turn-anchor">
        <Message from="assistant" className="fox-message fox-assistant-message">
          <MessageContent className="fox-assistant-content">
            {(state === 'running' || state === 'complete') && <AssistantProcess running={running} />}
            {state === 'question' ? null : state === 'error' ? <ErrorPrompt onRetry={onRetry} /> : state === 'denied' ? <DeniedPrompt /> : state === 'approval' ? null : running ? (
              <div className="fox-live-response"><FoxAssistantAvatar /><RunStatusText text="正在连接知识库并整理结果" /><i><b /><b /><b /></i></div>
            ) : <>
              <div className="fox-answer-body">
                <h2>Kun 的许可证</h2>
                <p><LicenseInlineCitation />，允许个人学习、研究、实验和其他非商业用途，也可以在许可范围内进行二次开发。</p>
                <MarkdownResponse className="fox-answer-response">{licenseAnswerMarkdown}</MarkdownResponse>
              </div>
              <Sources defaultOpen className="fox-sources">
              <SourcesTrigger count={citations.length}>引用了 {citations.length} 个来源<ChevronDown size={14} /></SourcesTrigger>
              <SourcesContent>
                {citations.map((item, index) => (
                  <Source key={item.id} href="#" title={item.title} onClick={(event) => event.preventDefault()}>
                    <span className="fox-citation-index">{index + 1}</span>
                    <span><strong>{item.title}</strong><small>{item.detail}</small></span>
                    <ChevronRight size={14} />
                  </Source>
                ))}
              </SourcesContent>
              </Sources>
            </>}
          </MessageContent>
          {state === 'complete' && <MessageActions className="fox-message-actions">
            <MessageAction tooltip="复制" onClick={() => { void navigator.clipboard?.writeText('Kun 使用 PolyForm Noncommercial License 1.0.0，企业商业使用需要单独授权。'); toast.success('已复制回复') }}><Copy size={14} /></MessageAction>
          </MessageActions>}
        </Message>
        </div>
      </ConversationContent>
      <ConversationBottomDock plan={latestPlanRevision} />
    </Conversation>
  )
}

export const MemoizedTimeline = memo(Timeline)

export function ConversationTurnRail({ messages, artifacts }: { messages: ConversationMessage[]; artifacts: ArtifactRecord[] }) {
  const [hoveredId, setHoveredId] = useState<string | null>(null)
  const [activeId, setActiveId] = useState<string | null>(null)
  const railRef = useRef<HTMLElement | null>(null)
  const turns = useMemo(() => messages.reduce<Array<{ user: ConversationMessage; assistant?: ConversationMessage; artifacts: ArtifactRecord[]; url?: string }>>((items, message) => {
    if (message.role === 'user') {
      items.push({ user: message, artifacts: [] })
      return items
    }
    if (message.role === 'assistant' && items.length) {
      const current = items.at(-1)!
      if (!current.assistant) current.assistant = message
    }
    return items
  }, []).map((turn) => {
    const content = `${turn.user.content}\n${turn.assistant?.content ?? ''}`
    return {
      ...turn,
      artifacts: artifacts.filter((artifact) => Boolean(turn.assistant?.runId) && artifact.runId === turn.assistant?.runId).slice(0, 2),
      url: content.match(/https?:\/\/[^\s)]+/)?.[0],
    }
  }), [artifacts, messages])

  useEffect(() => {
    if (!turns.length) {
      setActiveId(null)
      return
    }
    setActiveId((current) => current && turns.some((turn) => turn.user.id === current) ? current : turns.at(-1)!.user.id)
    const root = railRef.current?.closest('.fox-conversation') ?? null
    const anchors = turns.flatMap((turn) => {
      const anchor = document.getElementById(`fox-turn-${turn.user.id}`)
      return anchor ? [anchor] : []
    })
    if (!root || !anchors.length) return
    const visible = new Map<string, number>()
    const observer = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        const id = (entry.target as HTMLElement).dataset.turnMessageId
        if (!id) return
        if (entry.isIntersecting) visible.set(id, entry.intersectionRatio)
        else visible.delete(id)
      })
      const current = [...visible.entries()].sort((left, right) => right[1] - left[1])[0]?.[0]
      if (current) setActiveId(current)
    }, { root, threshold: [0.05, 0.25, 0.5, 0.8] })
    anchors.forEach((anchor) => observer.observe(anchor))
    return () => observer.disconnect()
  }, [turns])

  if (!turns.length) return null
  const hovered = turns.find((turn) => turn.user.id === hoveredId)
  const scrollToTurn = (messageId: string) => {
    setActiveId(messageId)
    document.getElementById(`fox-turn-${messageId}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  return <nav ref={railRef} className="fox-conversation-rail" aria-label="当前对话历史导航" onMouseLeave={() => setHoveredId(null)}>
    <div className="fox-conversation-marks">{turns.map((turn, index) => <button type="button" key={turn.user.id} className={turn.user.id === activeId ? 'is-active' : ''} aria-label={`第 ${index + 1} 轮：${turn.user.content.slice(0, 40)}`} onMouseEnter={() => setHoveredId(turn.user.id)} onFocus={() => setHoveredId(turn.user.id)} onBlur={() => setHoveredId(null)} onClick={() => scrollToTurn(turn.user.id)}><span /></button>)}</div>
    <div className={`fox-conversation-preview ${hovered ? 'is-visible' : ''}`}>
      <header><span><MessageCircleMore /></span><div><strong>{hovered?.user.content.slice(0, 58) || '对话回合'}</strong><small>{hovered ? `第 ${turns.indexOf(hovered) + 1} 轮 · ${new Date(hovered.user.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : ''}</small></div></header>
      <div className="fox-conversation-preview-answer"><small>Fox 回答</small><p>{splitAssistantContent(hovered?.assistant?.content ?? '').answer.slice(0, 180) || '正在等待 Fox 回答…'}</p></div>
      <div className="fox-conversation-resources">
        {hovered?.artifacts.map((artifact) => <span key={artifact.id}>{artifact.mediaType === 'text/html' || /html|web/i.test(artifact.artifactType) ? <Globe2 /> : artifact.mediaType?.startsWith('image/') ? <ImagePlus /> : <FileText />}<b>{artifact.displayName}</b></span>)}
        {hovered?.url && <span><Globe2 /><b>{hovered.url.replace(/^https?:\/\//, '').split('/')[0]}</b></span>}
        {hovered && hovered.artifacts.length === 0 && !hovered.url && <span className="is-fallback"><Sparkles /><b>Fox，你的智能助手</b></span>}
      </div>
    </div>
  </nav>
}
