import { AssistantRuntimeProvider } from '@assistant-ui/react'
import type { ArtifactRecord, ConversationMessage } from '../../conversations/model/types'
import { useDesktopConversation } from '../../conversations/hooks/use-desktop-conversation'
import type { RuntimeSource } from '../timeline/fox-runtime-process-model'
import type { GoalProgressData } from '../hooks/use-goal-progress'
import { TimelineRuntimeContext, ComposerRuntimeContext } from '../fox-shared'
import type { ExpertToolAvailability } from '../components/expert-binding-ui'
import type { ApprovalDecision } from '../../conversations/model/types'
import type { RuntimeQuestionRequest } from '@/features/conversations/model/pending-interactions'
import type { ApprovalRecord, ProjectRecord } from '@/features/conversations/model/types'
import type { AgentRecord } from '../../conversations/model/types'
import { useContext, useEffect, useMemo, useState } from 'react'
import FoxThread from './fox-thread'
import { FoxThreadComposer } from './composer/fox-thread-composer'
import { useFoxRuntime, type FoxRuntimeCallbacks } from './use-fox-runtime'
import { Button } from '@/components/ui/button'

type DesktopConversationState = ReturnType<typeof useDesktopConversation>

export interface AssistantUiChatProps {
  conversation: DesktopConversationState
  callbacks: FoxRuntimeCallbacks
  pendingMessage?: ConversationMessage | null
  isSendDisabled?: boolean
  /** Opening suggestions for the empty state (expert-defined). */
  suggestions?: string[]
  /** Latest plan revision for the bottom-dock floater. */
  planRevision?: GoalProgressData['planRevisions'][number]
  onOpenArtifact?: (artifact: ArtifactRecord) => void
  onOpenFileInSidebar?: (path: string) => void
  onRevealFileInExplorer?: (path: string) => void
  // --- composer (step 5D) -----------------------------------------------------
  projectRoot?: string | null
  projectPermissionMode?: ProjectRecord['permissionMode'] | null
  activeExpert?: AgentRecord | null
  expertReadOnly?: boolean
  expertToolAvailability?: ExpertToolAvailability
  supportsImageInput?: boolean
  composerDraft: string
  setComposerDraft: (value: string) => void
  composerResetKey: number
  steeringConversationId?: string
  steeringActive?: boolean
  questionRequest?: RuntimeQuestionRequest | null
  onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>
  runtimeApprovals?: ApprovalRecord[]
  onResolveApproval?: (approvalId: string, decision: ApprovalDecision) => void | Promise<boolean>
  goalProgressData?: GoalProgressData | null
  onResolveWorkModeConfirmation?: (goalId: string, expectedVersion: number, approved: boolean) => Promise<boolean> | void
  onResolvePlanRevision?: (planRevisionId: string, decision: 'approved' | 'rejected') => Promise<boolean> | void
  onResolveWorkflowGate?: (workflowRunId: string, stageId: string, decision: 'approved' | 'rejected') => Promise<boolean> | void
  onEvidenceClick?: (evidence: import('../../conversations/model/types').TaskEvidenceRecord) => void
  onProject?: () => void
  onPermissionModeChange?: (permissionMode: ProjectRecord['permissionMode']) => void | Promise<void>
  onViewExpert?: () => void
  onChangeExpert?: () => void
  onRemoveExpert?: () => void | Promise<void>
  onHeightChange?: (height: number) => void
  onCancel?: () => void | Promise<unknown>
}

/**
 * The assistant-ui chat branch (migration steps 5C/5D). Mounted only while the
 * `fox.ui.chatEngine` switch is set to `assistant-ui`, and loaded as its own
 * lazy chunk so the legacy path and the first-screen budget stay untouched.
 *
 * This container owns the runtime and the Fox thread context (raw run events,
 * attachment names, host actions); the message list lives in `fox-thread.tsx`
 * and the composer in `thread/composer/`.
 */
export default function AssistantUiChat({ conversation, callbacks, pendingMessage, isSendDisabled, suggestions, planRevision, onOpenArtifact, onOpenFileInSidebar, onRevealFileInExplorer, projectRoot, projectPermissionMode, activeExpert, expertReadOnly, expertToolAvailability, supportsImageInput, composerDraft, setComposerDraft, composerResetKey, steeringConversationId, steeringActive, questionRequest, onSubmitQuestion, runtimeApprovals, onResolveApproval, goalProgressData, onResolveWorkModeConfirmation, onResolvePlanRevision, onResolveWorkflowGate, onEvidenceClick, onProject, onPermissionModeChange, onViewExpert, onChangeExpert, onRemoveExpert, onHeightChange, onCancel }: AssistantUiChatProps) {
  const runtime = useFoxRuntime(conversation, callbacks, { pendingMessage, isSendDisabled })
  // The workbench provides knowledge bindings and the open-source action through
  // this context for the whole chat stage; reuse them instead of new props.
  const timelineContext = useContext(TimelineRuntimeContext)
  // The pending question round-trips through the composer context, exactly like
  // the legacy composer read it.
  const composerContext = useContext(ComposerRuntimeContext)
  const detail = conversation.detail
  // Tool rows and the process group read the raw run events; grouping once per
  // detail change keeps every message's event array reference stable.
  const events = useMemo(() => detail?.runtimeEvents ?? [], [detail])
  const eventsByRun = useMemo(() => {
    const grouped = new Map<string, typeof events>()
    for (const event of events) {
      const bucket = grouped.get(event.runId)
      if (bucket) bucket.push(event)
      else grouped.set(event.runId, [event])
    }
    return grouped
  }, [events])
  const attachments = useMemo(() => detail?.attachments ?? [], [detail])
  const attachmentNames = useMemo(() => new Map(attachments.map((attachment) => [attachment.id, attachment.displayName])), [attachments])
  const attachmentsByMessage = useMemo(() => {
    const grouped = new Map<string, typeof attachments>()
    for (const attachment of attachments) {
      if (!attachment.messageId) continue
      const bucket = grouped.get(attachment.messageId)
      if (bucket) bucket.push(attachment)
      else grouped.set(attachment.messageId, [attachment])
    }
    return grouped
  }, [attachments])
  // Same lazy load as the legacy timeline: grouping helpers arrive only when
  // the conversation actually carries runtime events.
  const [groupHelpers, setGroupHelpers] = useState<typeof import('../runtime-process-groups') | null>(null)
  const hasRuntimeEvents = events.length > 0
  useEffect(() => {
    if (!hasRuntimeEvents) return
    let mounted = true
    void import('../runtime-process-groups').then((helpers) => { if (mounted) setGroupHelpers(helpers) })
    return () => { mounted = false }
  }, [hasRuntimeEvents])
  const fox = useMemo(() => ({
    events,
    eventsByRun,
    attachmentNames,
    attachmentsByMessage,
    knowledgeBindings: timelineContext.knowledgeBindings,
    groupHelpers,
    onOpenSource: timelineContext.onOpenSource,
    onOpenArtifact,
    onOpenFileInSidebar,
    onRevealFileInExplorer,
  }), [events, eventsByRun, attachmentNames, attachmentsByMessage, timelineContext.knowledgeBindings, timelineContext.onOpenSource, groupHelpers, onOpenArtifact, onOpenFileInSidebar, onRevealFileInExplorer])
  const runtimeInitializing = conversation.enabled && !conversation.ready && !conversation.error
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <FoxThread fox={fox} suggestions={suggestions} planRevision={planRevision} turnRailMessages={detail?.messages} turnRailArtifacts={detail?.artifacts} />
      <FoxThreadComposer
        runtimeControlled={conversation.enabled}
        runtimeInitializing={runtimeInitializing}
        projectRoot={projectRoot}
        projectPermissionMode={projectPermissionMode}
        activeExpert={activeExpert}
        expertReadOnly={expertReadOnly}
        expertToolAvailability={expertToolAvailability}
        supportsImageInput={supportsImageInput}
        draft={composerDraft}
        setDraft={setComposerDraft}
        resetKey={composerResetKey}
        steeringConversationId={steeringConversationId}
        steeringActive={steeringActive}
        questionRequest={questionRequest ?? composerContext.questionRequest}
        onSubmitQuestion={onSubmitQuestion ?? composerContext.onSubmitQuestion}
        runtimeApprovals={runtimeApprovals}
        onResolveApproval={onResolveApproval}
        goalProgressData={goalProgressData}
        onResolveWorkModeConfirmation={onResolveWorkModeConfirmation}
        onResolvePlanRevision={onResolvePlanRevision}
        onResolveWorkflowGate={onResolveWorkflowGate}
        onEvidenceClick={onEvidenceClick}
        onProject={onProject}
        onPermissionModeChange={onPermissionModeChange}
        onKnowledge={undefined}
        onViewExpert={onViewExpert}
        onChangeExpert={onChangeExpert}
        onRemoveExpert={onRemoveExpert}
        onHeightChange={onHeightChange}
        onCancel={onCancel}
      />
    </AssistantRuntimeProvider>
  )
}

/** Temporary engine switch for the development rounds; removed when the
 * switch default flips in 5E. */
export function ChatEngineSwitch({ engine, onChange }: { engine: 'legacy' | 'assistant-ui'; onChange: (engine: 'legacy' | 'assistant-ui') => void }) {
  return (
    <Button variant="ghost" size="sm" className="fox-thread-engine-switch" onClick={() => onChange(engine === 'legacy' ? 'assistant-ui' : 'legacy')}>
      {engine === 'legacy' ? '试用 assistant-ui 界面' : '返回经典界面'}
    </Button>
  )
}
