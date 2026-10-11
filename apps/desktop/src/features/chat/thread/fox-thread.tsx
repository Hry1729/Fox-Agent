import { ThreadPrimitive, useAuiState } from '@assistant-ui/react'
import { type GoalProgressData } from '../hooks/use-goal-progress'
import type { ArtifactRecord, ConversationMessage } from '../../conversations/model/types'
import { ConversationEmptyPrompt, ConversationTurnRail } from '../timeline/fox-timeline'
import { FoxThinkingDots } from '../components/FoxThinkingDots'
import { ListTodo } from 'lucide-react'
import { FoxAssistantMessage } from './fox-assistant-message'
import { FoxThreadContext, type FoxThreadContextValue } from './fox-thread-context'
import { FoxUserMessage } from './fox-user-message'
import './fox-thread.css'

export interface FoxThreadProps {
  fox: FoxThreadContextValue
  /** Opening suggestions for the empty state (expert-defined or default). */
  suggestions?: string[]
  /** Fills the composer draft when an empty-state suggestion is clicked. */
  onStart?: (suggestion: string) => void
  /** Latest plan revision, shown as the bottom-dock floater. */
  planRevision?: GoalProgressData['planRevisions'][number]
  /** Raw conversation rows for the turn rail (id anchors live on messages). */
  turnRailMessages?: ConversationMessage[]
  turnRailArtifacts?: ArtifactRecord[]
}

function FoxThreadLoadEarlier() {
  const hasEarlier = useAuiState((state) => state.thread.hasEarlier)
  if (!hasEarlier) return null
  return (
    <ThreadPrimitive.LoadEarlier className="fox-thread-load-earlier">
      加载更早消息
    </ThreadPrimitive.LoadEarlier>
  )
}

function FoxScrollToBottom() {
  const isRunning = useAuiState((state) => state.thread.isRunning)
  const isEmpty = useAuiState((state) => state.thread.isEmpty)
  if (isEmpty) return null
  // ScrollToBottom 自己会在"已经到底"时返回 null，所以这里只决定形态：思考中给点阵，
  // 空闲给箭头。以前运行中直接隐藏，恰好把最需要它的场景（被新内容吸走又看不到）拿掉了。
  return (
    <ThreadPrimitive.ScrollToBottom className="fox-scroll-button" aria-label="定位到最新消息">
      {isRunning ? <FoxThinkingDots /> : <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>}
    </ThreadPrimitive.ScrollToBottom>
  )
}

/** The bottom dock: scroll-to-bottom button plus the plan floater, mirroring
 * the legacy ConversationBottomDock. The button reads the run state from the AUI
 * store itself, so the dock needs no extra prop. */
function FoxBottomDock({ plan }: { plan?: GoalProgressData['planRevisions'][number] }) {
  const planStatus = plan?.status === 'proposed'
    ? '待批准'
    : plan?.status === 'approved'
      ? '已批准'
      : plan?.status === 'rejected'
        ? '已退回'
        : '已更新'
  return (
    <div className={`fox-conversation-bottom-dock ${plan ? 'has-plan' : ''}`}>
      <FoxScrollToBottom />
      {plan && <div className={`fox-plan-floater is-${plan.status}`} role="status" title={[plan.title, plan.summary].filter(Boolean).join('\n')}>
        <ListTodo size={12} />
        <strong>{plan.title || `计划 v${plan.revision}`}</strong>
        <span>v{plan.revision} · {planStatus}</span>
      </div>}
    </div>
  )
}

/**
 * The assistant-ui message list (migration step 5C). Must render inside an
 * `AssistantRuntimeProvider`; `ThreadPrimitive.Root` is keyed by conversation id
 * at the mount site, so switching conversations rebuilds the runtime state
 * instead of mixing message streams.
 */
export default function FoxThread({ fox, suggestions, onStart, planRevision, turnRailMessages, turnRailArtifacts }: FoxThreadProps) {
  return (
    <FoxThreadContext.Provider value={fox}>
      <div className="fox-thread">
        <ConversationTurnRail messages={turnRailMessages ?? []} artifacts={turnRailArtifacts ?? []} />
        <ThreadPrimitive.Root className="fox-thread-root">
          <ThreadPrimitive.Viewport className="fox-thread-viewport" autoScroll>
            {/* Hybrid rollback: the empty conversation keeps the classic
                mascot + suggestions screen from the legacy timeline. */}
            <ThreadPrimitive.Empty>
              <ConversationEmptyPrompt suggestions={suggestions} onStart={onStart ?? (() => undefined)} />
            </ThreadPrimitive.Empty>
            <FoxThreadLoadEarlier />
            <ThreadPrimitive.If empty={false}>
              <div className="fox-thread-date"><span>今天</span></div>
              <ThreadPrimitive.Messages components={{ UserMessage: FoxUserMessage, AssistantMessage: FoxAssistantMessage }} />
            </ThreadPrimitive.If>
          </ThreadPrimitive.Viewport>
          <FoxBottomDock plan={planRevision} />
        </ThreadPrimitive.Root>
      </div>
    </FoxThreadContext.Provider>
  )
}
