import { ExpertBindingChip } from '../components/ExpertBindingChip'
import { ComposerProjectBar } from './composer-project-bar'
import { ComposerAgentPicker } from './composer-agent-picker'
import { ComposerEntryContext } from './composer-entry-context'
import { ComposerModelPicker } from './composer-model-picker'
import { GoalProgress } from '../components/GoalProgress'
import { ManagedFilesPanel } from '../components/ManagedFilesPanel'
import { RunSteeringStrip } from '../components/RunSteeringStrip'
import { type ExpertToolAvailability } from '../components/expert-binding-ui'
import { steeringComposerView, steeringSubmitMode } from '../components/steering-presentation'
import { type ExecutionReceiptPresentation, executionReceiptPresentation } from '../execution-receipt'
import { type GoalProgressData } from '../hooks/use-goal-progress'
import { useRunSteering } from '../hooks/use-run-steering'
import { localKnowledgePickerState } from '../knowledge-picker-state'
import { MAX_COMPOSER_ATTACHMENT_BYTES } from '../local-knowledge-document-draft'
import { ComposerInitialAttachmentBridge, type ComposerInitialAttachments } from './composer-initial-attachments'
import { conversationModelOverride, yuxiModelOverrideAllowed } from '../model-selection'
import { ComposerMenu, Models } from '@/components/assistant-ui/elements/composer'
import { useMentionFiles } from './use-mention-files'
import { ComposerCommandItem, ComposerPersonItem, useMentionMatches, applyMention as insertMention } from '@/components/assistant-ui/upstream/elements/composer'
import { CommandItem as CommandPrimitiveItem } from 'cmdk'
import { Attachment, AttachmentInfo, AttachmentPreview, AttachmentRemove, Attachments } from '@/components/ai-elements/attachments'
import { Confirmation, ConfirmationAction, ConfirmationActions, ConfirmationRequest, ConfirmationTitle } from '@/components/ai-elements/confirmation'
import { PromptInput, SEND_NOT_STARTED_MESSAGE, PromptInputActionAddAttachments, PromptInputActionMenu, PromptInputActionMenuContent, PromptInputActionMenuItem, PromptInputActionMenuTrigger, PromptInputBody, PromptInputFooter, PromptInputSelect, PromptInputSelectContent, PromptInputSelectItem, PromptInputSelectTrigger, PromptInputSelectValue, PromptInputSubmit, PromptInputTextarea, PromptInputTools, usePromptInputAttachments } from '@/components/ai-elements/prompt-input'
import { Suggestion, Suggestions } from '@/components/ai-elements/suggestion'
import { BorderBeam } from '@/components/effects/border-beam'
import { Button } from '@/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { type RunSteeringLane, knowledgeReferenceFromLegacyBinding, knowledgeReferenceKey } from '@/features/conversations/api/desktop-client'
import { type RuntimeQuestion, type RuntimeQuestionRequest } from '@/features/conversations/model/pending-interactions'
import { type AgentRecord, type ApprovalDecision, type ApprovalRecord, type KnowledgeBindingRecord, type KnowledgeReference, type ModelServiceRecord, type ProjectRecord, type TaskEvidenceRecord, type ToolCallRecord, type YuxiModelRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { SendNotStartedNotice } from './send-not-started-notice'
import { type ComposerAccessMode, FOX_ACCESS_MODES, accessModeLabel } from '../access-mode'
import { ArrowUp, Eye, Play, Check, ChevronDown, CircleHelp, ShieldAlert, CircleStop, Clock3, CornerDownLeft, FileText, FolderOpen, Library, LoaderCircle, Plus, Sparkles, Terminal, X } from 'lucide-react'
import { type ChangeEvent, type ComponentProps, type Dispatch, type KeyboardEvent, type ReactNode, type SetStateAction, Suspense, lazy, memo, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { type SlashCommand, slashCommandRuntimeText, slashCommands } from './fox-slash-commands'
import { ApprovalPrompt, QuestionPrompt } from '../demo/fox-demo-prompts'
import { type ChatState, ComposerRuntimeContext, isConfirmationQuestion, layoutStorage } from '../fox-shared'
import { compactValue } from '../timeline/fox-runtime-process-model'

const RuntimeApprovalPrompt = lazy(() => import('../components/RuntimeApprovalPrompt').then(module => ({ default: module.RuntimeApprovalPrompt })))

function ComposerBorderBeam({ enabled, ...props }: ComponentProps<typeof BorderBeam> & { enabled: boolean }) {
  return enabled ? <BorderBeam {...props} /> : <>{props.children}</>
}

function ComposerAttachmentPreview({ knowledgeChips = [], onKnowledge }: { knowledgeChips?: Array<{ key: string; name: string }>; onKnowledge?: () => void }) {
  const attachments = usePromptInputAttachments()
  const items: ReactNode[] = []
  knowledgeChips.forEach((chip) => {
    const label = chip.name
    items.push(<button type="button" key={chip.key} className="is-authorized is-knowledge" onClick={onKnowledge} title={label}><Library size={13} /><span>{label}</span><Check size={12} /></button>)
  })
  attachments.files.forEach((file) => items.push(<Attachment key={file.id} data={file} onRemove={() => attachments.remove(file.id)}><AttachmentPreview /><AttachmentInfo /><AttachmentRemove label="移除附件" /></Attachment>))
  if (!items.length) return null
  const labels = [...knowledgeChips.map((chip) => chip.name), ...attachments.files.map((file) => file.filename || '附件')]
  return (
    <Attachments variant="inline" className="fox-composer-attachments fox-composer-context fox-composer-selection-row">
      {items.slice(0, 3)}
      {items.length > 3 && <Popover><Tooltip><TooltipTrigger asChild><PopoverTrigger asChild><button type="button" className="fox-composer-overflow" aria-label={`查看其余 ${items.length - 3} 项`}>…</button></PopoverTrigger></TooltipTrigger><TooltipContent className="max-h-64 overflow-auto">{labels.slice(3).map((label, index) => <p key={index}>{label}</p>)}</TooltipContent></Tooltip><PopoverContent className="fox-composer-overflow-list" align="start">{items.slice(3)}</PopoverContent></Popover>}
    </Attachments>
  )
}

function ComposerAttachmentCommandBridge({ openRef }: { openRef: { current: () => void } }) {
  const attachments = usePromptInputAttachments()

  useEffect(() => {
    openRef.current = attachments.openFileDialog
    return () => {
      openRef.current = () => undefined
    }
  }, [attachments.openFileDialog, openRef])

  return null
}

// The tool-name classifiers live in a dependency-free module; re-exported here
// so existing imports from the composer keep working.
export { isFileChangeTool, isCommandTool, isChangeOrCommandTool } from './fox-tool-classification'

function toolResultDetails(tool: ToolCallRecord) {
  const result = tool.result && typeof tool.result === 'object' && !Array.isArray(tool.result)
    ? tool.result as Record<string, unknown>
    : {}
  const details = result.details && typeof result.details === 'object' && !Array.isArray(result.details)
    ? result.details as Record<string, unknown>
    : result
  return { result, details }
}

export function toolExecutionStatus(tool: ToolCallRecord): ExecutionReceiptPresentation | null {
  return executionReceiptPresentation(tool.result)
}

export function toolStatusLabel(tool: ToolCallRecord, execution = toolExecutionStatus(tool)) {
  if (execution) return execution.statusLabel
  return tool.status === 'completed' ? '已完成' : tool.status === 'failed' ? '失败' : '处理中'
}

export function commandOutput(tool: ToolCallRecord) {
  const { result, details } = toolResultDetails(tool)
  const input = tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input)
    ? tool.input as Record<string, unknown>
    : {}
  const stdout = compactValue(details.stdout ?? result.stdout)
  const stderr = compactValue(details.stderr ?? result.stderr)
  const output = [stdout && `stdout:\n${stdout}`, stderr && `stderr:\n${stderr}`].filter(Boolean).join('\n\n')
  const exitCode = details.exitCode ?? result.exitCode
  const cwd = compactValue(details.cwd ?? input.cwd)
  const duration = tool.completedAt !== null
    ? Math.max(0, tool.completedAt - tool.startedAt)
    : null
  return { output, exitCode: typeof exitCode === 'number' ? exitCode : null, cwd, duration, execution: toolExecutionStatus(tool) }
}

export function readStoredSeenChanges() {
  if (typeof window === 'undefined') return {} as Record<string, number>
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(layoutStorage.seenChanges) ?? '{}')
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => Number.isFinite(entry[1]) && entry[1] >= 0))
  } catch {
    return {}
  }
}

function Composer({ topSlot, inputLockedReason = null, initialAttachments, onInitialAttachmentsConsumed, resetKey, draft, setDraft, chatState, centered = false, newConversation, dark = false, runtimeControlled = false, runtimeInitializing = false, projectRoot, projectPermissionMode, activeAgent, activeExpert, expertReadOnly = false, expertToolAvailability, agents = [], modelService, runtimeCapabilities, yuxiModels = [], knowledgeBindings = [], questionRequest, goalProgressData, runtimeApprovals = [], steeringConversationId, steeringActive = false, onProject, onPermissionModeChange, onKnowledge, onAgentChange, onViewExpert, onChangeExpert, onRemoveExpert, onHeightChange, onPromptCommit, onSubmitPrompt, onSubmitQuestion, onApprove, onDeny, onAnswer, onResolveApproval, onResolveWorkModeConfirmation, onResolvePlanRevision, onResolveWorkflowGate, onEvidenceClick, onStatusChange, onCancel, sendFailureReason }: { /** Row rendered inside the composer container, directly above the input box (Agent backend bar). */ topSlot?: ReactNode; /** When set, the conversation is read-only: input and send are disabled with this reason. */ inputLockedReason?: string | null; initialAttachments?: ComposerInitialAttachments | null; onInitialAttachmentsConsumed?: (requestId: number) => void; resetKey: number; draft: string; setDraft: Dispatch<SetStateAction<string>>; chatState: ChatState; centered?: boolean; newConversation?: boolean; dark?: boolean; runtimeControlled?: boolean; runtimeInitializing?: boolean; projectRoot?: string | null; projectPermissionMode?: ComposerAccessMode | null; activeAgent?: AgentRecord | null; activeExpert?: AgentRecord | null; expertReadOnly?: boolean; expertToolAvailability?: ExpertToolAvailability; agents?: AgentRecord[]; modelService?: ModelServiceRecord | null; runtimeCapabilities?: Record<string, unknown>; yuxiModels?: YuxiModelRecord[]; knowledgeBindings?: KnowledgeBindingRecord[]; questionRequest?: RuntimeQuestionRequest | null; goalProgressData?: GoalProgressData | null; runtimeApprovals?: ApprovalRecord[]; steeringConversationId?: string; steeringActive?: boolean; onProject?: () => void; onPermissionModeChange?: (permissionMode: ComposerAccessMode) => void | Promise<void>; onKnowledge?: () => void; onAgentChange?: (agentId: string) => void; onViewExpert?: () => void; onChangeExpert?: () => void; onRemoveExpert?: () => void | Promise<void>; onHeightChange?: (height: number) => void; onPromptCommit?: (prompt: string) => void; onSubmitPrompt?: (prompt: string, model?: string, files?: Array<{ filename?: string; mediaType?: string; url?: string }>, runtimeText?: string) => 'complete' | 'question' | 'approval' | 'error' | void | Promise<'complete' | 'question' | 'approval' | 'error' | void>; onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>; onApprove?: () => void; onDeny?: () => void; onAnswer?: (answer: string) => void; onResolveApproval?: (approvalId: string, decision: ApprovalDecision) => void | Promise<boolean>; onResolveWorkModeConfirmation?: (goalId: string, expectedVersion: number, approved: boolean) => Promise<boolean>; onResolvePlanRevision?: (planRevisionId: string, decision: 'approved' | 'rejected') => Promise<boolean>; onResolveWorkflowGate?: (workflowRunId: string, stageId: string, decision: 'approved' | 'rejected') => Promise<boolean>; onEvidenceClick?: (evidence: TaskEvidenceRecord) => void; onStatusChange?: (status: 'ready' | 'streaming') => void; onCancel?: () => boolean | void | Promise<boolean | void>; /** Why the last send did not start (Host error), read right after onSubmitPrompt returns 'error'. */ sendFailureReason?: () => string | null | undefined }) {
  const runtimeContext = useContext(ComposerRuntimeContext)
  const entry = useContext(ComposerEntryContext)
  const displayExpert = activeExpert ?? entry.defaultExpert
  const resolveRuntimeApproval = onResolveApproval
  const activeQuestionRequest = questionRequest ?? runtimeContext.questionRequest
  const submitQuestion = onSubmitQuestion ?? runtimeContext.onSubmitQuestion
  const activeKnowledgeBindings = knowledgeBindings.length ? knowledgeBindings : runtimeContext.knowledgeBindings
  const enabledKnowledgeBindings = activeKnowledgeBindings.filter((binding) => binding.enabled !== false)
  const configuredKnowledgeReferences = Array.isArray(runtimeContext.knowledgeReferences)
    ? runtimeContext.knowledgeReferences
    : enabledKnowledgeBindings.map(knowledgeReferenceFromLegacyBinding)
  const selectedKnowledgeReferenceKeys = new Set(configuredKnowledgeReferences.map(knowledgeReferenceKey))
  const knowledgeChips = configuredKnowledgeReferences.map((reference) => ({
    key: knowledgeReferenceKey(reference),
    name: runtimeContext.knowledgeReferenceNames?.[knowledgeReferenceKey(reference)]
      ?? (reference.source === 'local' ? runtimeContext.localKnowledgeBases?.find((item) => item.id === reference.id)?.name
        : runtimeContext.remoteKnowledgeBases?.find((item) => item.id === reference.id)?.name)
      ?? enabledKnowledgeBindings.find((binding) => knowledgeReferenceKey(knowledgeReferenceFromLegacyBinding(binding)) === knowledgeReferenceKey(reference))?.knowledgeBaseName
      ?? reference.id,
  }))
  const openKnowledge = onKnowledge ?? runtimeContext.onKnowledge
  const [status, setStatus] = useState<'ready' | 'streaming'>('ready')
  // 「发送未开始」 with the Host's reason, shown above the input until the next send.
  const [sendNotStarted, setSendNotStarted] = useState<{ message: string; detail?: string } | null>(null)
  const [focused, setFocused] = useState(false)
  const spellcheckEnabled = typeof window === 'undefined' || window.localStorage.getItem('fox.preferences.spellcheck') !== '0'
  const allowYuxiModelOverride = yuxiModelOverrideAllowed(activeAgent)
  const isYuxiAgent = activeAgent?.runtimeType === 'yuxi'
  const supportsImageInput = isYuxiAgent || runtimeCapabilities?.imageInput === true
  const supportsDynamicModelSwitch = isYuxiAgent
    ? allowYuxiModelOverride
    : runtimeCapabilities?.dynamicModelSwitch === true
  const modelOptions = activeAgent?.runtimeType === 'yuxi'
    ? allowYuxiModelOverride ? yuxiModels : []
    : modelService?.modelId
      ? [{ spec: modelService.modelId, modelId: modelService.modelId, displayName: modelService.modelId, providerId: 'fox', providerDisplayName: modelService.name }]
      : []
  const defaultModel = activeAgent?.runtimeType === 'yuxi'
    ? activeAgent.defaultModel
    : modelService?.modelId ?? (activeAgent?.defaultModel !== 'configured-model' ? activeAgent?.defaultModel : undefined) ?? '未配置模型'
  const [model, setModel] = useState(defaultModel)
  const [questionFreeform, setQuestionFreeform] = useState('')
  const [questionSelections, setQuestionSelections] = useState<Record<string, string[]>>({})
  const [questionOtherAnswers, setQuestionOtherAnswers] = useState<Record<string, string>>({})
  const [commandOpen, setCommandOpen] = useState(false)
  const [commandView, setCommandView] = useState<'commands' | 'knowledge'>('commands')
  const [selectedSlashCommand, setSelectedSlashCommand] = useState<SlashCommand | null>(null)
  const [mentionOpen, setMentionOpen] = useState(false)
  const [commandHighlight, setCommandHighlight] = useState('目标')
  const [mentionHighlight, setMentionHighlight] = useState('')
  const [steeringStripOpen, setSteeringStripOpen] = useState(false)
  const submitTimer = useRef<number | null>(null)
  const cancelPendingRef = useRef(false)
  const questionSubmittingRef = useRef(false)
  const composerRef = useRef<HTMLDivElement | null>(null)
  const promptRef = useRef<HTMLDivElement | null>(null)
  const commandMenuRef = useRef<HTMLDivElement | null>(null)
  const mentionMenuRef = useRef<HTMLDivElement | null>(null)
  const mentionChoices = useMentionFiles(mentionOpen, steeringConversationId, runtimeContext.mentionFiles)
  const mentionQuery = draft.match(/@([^\s@]*)$/u)?.[1].toLocaleLowerCase() ?? ''
  const mentionFiles = useMentionMatches(draft, mentionChoices.files.map(name => ({ name, role: 'file' as const })))
  const commandQuery = draft.trimStart().slice(1).toLocaleLowerCase()
  const matchingCommands = slashCommands.filter(command => command.id.toLocaleLowerCase().startsWith(commandQuery))
  useEffect(() => {
    if (commandView === 'commands' && !matchingCommands.some(command => command.id === commandHighlight)) setCommandHighlight(matchingCommands[0]?.id ?? '')
  }, [commandView, matchingCommands, commandHighlight])
  useEffect(() => {
    if (!mentionFiles.some(person => person.name === mentionHighlight)) setMentionHighlight(mentionFiles[0]?.name ?? '')
  }, [mentionFiles, mentionHighlight])
  const attachmentDialogRef = useRef<() => void>(() => undefined)

  useEffect(() => () => {
    if (submitTimer.current !== null) window.clearTimeout(submitTimer.current)
  }, [])

  useEffect(() => {
    if (submitTimer.current !== null) {
      window.clearTimeout(submitTimer.current)
      submitTimer.current = null
    }
    setStatus('ready')
    setCommandOpen(false)
    setCommandView('commands')
    setSelectedSlashCommand(null)
    setMentionOpen(false)
  }, [resetKey])

  useEffect(() => {
    setModel(defaultModel)
  }, [defaultModel, activeAgent?.id])

  useEffect(() => {
    setQuestionSelections({})
    setQuestionOtherAnswers({})
    setQuestionFreeform('')
    questionSubmittingRef.current = false
    if (activeQuestionRequest) setStatus('ready')
  }, [activeQuestionRequest?.runId])

  useEffect(() => {
    if (!runtimeControlled) return
    setStatus(chatState === 'running' ? 'streaming' : 'ready')
  }, [chatState, runtimeControlled])

  useEffect(() => {
    const composer = composerRef.current
    const prompt = promptRef.current
    const stage = composer?.closest<HTMLElement>('.fox-chat-stage')
    if (!composer || !prompt || !stage) return

    let animationFrame = 0
    const updateHeight = () => {
      window.cancelAnimationFrame(animationFrame)
      animationFrame = window.requestAnimationFrame(() => {
        stage.style.setProperty('--fox-empty-composer-height', `${composer.getBoundingClientRect().height}px`)
        const heading = stage.querySelector('.fox-empty-heading')
        if (heading) stage.style.setProperty('--fox-empty-heading-height', `${heading.getBoundingClientRect().height}px`)
        const stageBottom = stage.getBoundingClientRect().bottom
        const composerTop = composer.getBoundingClientRect().top
        onHeightChange?.(Math.ceil(Math.max(0, stageBottom - composerTop + 6)))
      })
    }

    updateHeight()
    const observer = new ResizeObserver(updateHeight)
    observer.observe(stage)
    observer.observe(composer)
    observer.observe(prompt)
    const heading = stage.querySelector('.fox-empty-heading')
    if (heading) observer.observe(heading)
    window.addEventListener('resize', updateHeight)

    return () => {
      window.cancelAnimationFrame(animationFrame)
      observer.disconnect()
      window.removeEventListener('resize', updateHeight)
    }
  }, [onHeightChange])

  useLayoutEffect(() => {
    const composer = composerRef.current
    const textarea = composer?.querySelector('textarea')
    if (!composer || !textarea || newConversation) return
    let frame = 0
    const measure = () => {
      composer.classList.remove('is-expanded')
      textarea.style.height = '0px'
      const multiline = textarea.scrollHeight > 34
      const hasAttachments = Boolean(composer.querySelector('.fox-composer-attachments > *'))
      composer.classList.toggle('is-expanded', multiline || hasAttachments)
      textarea.style.height = '0px'
      const height = Math.min(140, Math.max(32, textarea.scrollHeight))
      textarea.style.height = `${height}px`
      textarea.style.overflowY = textarea.scrollHeight > 140 ? 'auto' : 'hidden'
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure) }
    measure()
    let width = composer.clientWidth
    const resize = new ResizeObserver(() => {
      if (width !== composer.clientWidth) { width = composer.clientWidth; schedule() }
    })
    resize.observe(composer)
    const mutations = new MutationObserver(schedule)
    mutations.observe(composer, { childList: true, subtree: true })
    return () => { cancelAnimationFrame(frame); resize.disconnect(); mutations.disconnect() }
  }, [draft, newConversation])

  const onDraftChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    const value = event.currentTarget.value
    const leadingValue = value.trimStart()
    const hasSlashCommand = /^\/[^\s]*$/u.test(leadingValue)
    setDraft(value)
    setCommandOpen(hasSlashCommand)
    if (hasSlashCommand) setCommandView('commands')
    setMentionOpen(!hasSlashCommand && /(^|\s)@[^\s@]*$/u.test(value))
  }

  const applySlashCommand = (command: SlashCommand) => {
    setDraft((value) => value.replace(/^\s*\/[^\s]*/u, '').trimStart())
    setMentionOpen(false)
    if (command.action === 'attachments') {
      setCommandOpen(false)
      setCommandView('commands')
      setSelectedSlashCommand(null)
      attachmentDialogRef.current()
      return
    }
    if (command.action === 'knowledge') {
      setSelectedSlashCommand(null)
      setCommandView('knowledge')
      setCommandOpen(true)
      runtimeContext.onKnowledgeMenuOpen?.()
      return
    }
    setCommandOpen(false)
    setCommandView('commands')
    setSelectedSlashCommand(command)
  }

  const applyMention = (path: string) => {
    setDraft(value => insertMention(value, path))
    setMentionOpen(false)
  }

  const onComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    // Only a bare Enter drives the slash menu. Ctrl/Cmd+Enter and Shift+Enter stay
    // with the textarea so they can break the line instead of picking a command.
    const menuEnter = event.key === 'Enter'
      && !event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey
      && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229
    if ((commandOpen || mentionOpen) && (event.key === 'ArrowDown' || event.key === 'ArrowUp' || menuEnter)) {
      event.preventDefault()
      const menu = commandOpen ? commandMenuRef : mentionMenuRef
      menu.current?.dispatchEvent(new globalThis.KeyboardEvent('keydown', {
        key: event.key,
        bubbles: true,
        cancelable: true,
      }))
      return
    }
    if (event.key === 'Escape') {
      setCommandOpen(false)
      setCommandView('commands')
      setMentionOpen(false)
    }
  }

  const selectQuestionOption = (question: RuntimeQuestion, value: string) => {
    setQuestionSelections((current) => {
      const selected = current[question.questionId] ?? []
      return {
        ...current,
        [question.questionId]: question.multiSelect
          ? selected.includes(value) ? selected.filter((item) => item !== value) : [...selected, value]
          : [value],
      }
    })
  }

  const questionSubmitting = Boolean(activeQuestionRequest && status === 'streaming')
  const buildQuestionAnswer = (otherText: string) => {
    if (!activeQuestionRequest) return null
    const answers: Record<string, string | string[]> = {}
    const lines: string[] = []
    for (const question of activeQuestionRequest.questions) {
      const selected = questionSelections[question.questionId] ?? []
      if (selected.length) {
        answers[question.questionId] = question.multiSelect ? selected : selected[0]
        const labels = selected.map((value) => question.options.find((option) => option.value === value)?.label ?? value)
        lines.push(`${question.question}：${labels.join('、')}`)
      } else {
        const individualAnswer = questionOtherAnswers[question.questionId]?.trim()
        const fallbackAnswer = activeQuestionRequest.questions.length === 1 ? otherText.trim() : ''
        const openAnswer = individualAnswer || fallbackAnswer
        if (!question.allowOther || !openAnswer) return null
        answers[question.questionId] = openAnswer
        lines.push(`${question.question}：${openAnswer}`)
      }
    }
    if (otherText.trim() && !Object.values(answers).some((value) => value === otherText.trim())) {
      lines.push(`其他补充：${otherText.trim()}`)
    }
    return { answers, text: lines.join('\n') }
  }

  const submitQuestionAnswer = async () => {
    if (!activeQuestionRequest || questionSubmittingRef.current) return
    const answer = buildQuestionAnswer(questionFreeform)
    if (!answer) {
      toast.error('请先完成需要补充的事项')
      return
    }
    questionSubmittingRef.current = true
    setStatus('streaming')
    const submitted = await submitQuestion?.(answer.text, answer.answers)
    if (!submitted) {
      questionSubmittingRef.current = false
      setStatus('ready')
      return
    }
    setQuestionFreeform('')
    setQuestionSelections({})
    setQuestionOtherAnswers({})
    onStatusChange?.('streaming')
  }

  const pendingApprovals = runtimeApprovals
    .filter((approval) => approval.status === 'pending')
    .sort((left, right) => left.requestedAt - right.requestedAt || left.id.localeCompare(right.id))
  const activeApproval = pendingApprovals[0]
  const demoApprovalPending = !runtimeControlled && chatState === 'approval' && Boolean(onApprove && onDeny)
  const demoQuestionPending = !runtimeControlled && chatState === 'question' && Boolean(onAnswer)
  const goalConfirmationPending = goalProgressData?.goal.status === 'proposed' && Boolean(onResolveWorkModeConfirmation)
  const planRevisionPending = Boolean(
    onResolvePlanRevision && goalProgressData?.planRevisions.some((plan) => plan.status === 'proposed'),
  )
  const workflowGatePending = Boolean(onResolveWorkflowGate && goalProgressData?.expertWorkflow?.gates.some((gate) => gate.status === 'pending'))
  const decisionPending = demoApprovalPending || demoQuestionPending || pendingApprovals.length > 0 || Boolean(activeQuestionRequest) || goalConfirmationPending || planRevisionPending || workflowGatePending
  const approvalDecisionPending = demoApprovalPending || pendingApprovals.length > 0

  // Mid-run supplementary requests ("运行中补充要求"). One hook owns the durable
  // listing and both submit lanes; the composer is the single editor for them.
  const steering = useRunSteering({
    conversationId: steeringConversationId,
    active: runtimeControlled && (steeringActive || status === 'streaming'),
    enabled: runtimeControlled,
  })
  const steeringView = useMemo(
    () =>
      steeringComposerView({
        active: runtimeControlled && (steeringActive || status === 'streaming'),
        runId: steering.runId,
        runState: steering.runState,
        acceptsSteering: steering.acceptsSteering,
        rows: steering.rows.map((row) => ({
          messageId: row.messageId,
          content: row.content,
          status: row.status,
          lane: row.lane,
        })),
      }),
    [runtimeControlled, steering.acceptsSteering, steering.rows, steering.runId, steering.runState, steeringActive, status],
  )
  // A decision (approval or question) always owns the composer: supplementary
  // text must never be able to stand in for an approval, and a Run parked on an
  // approval is not accepting steering input anyway.
  const steeringMode = steeringSubmitMode({
    runtimeControlled,
    status,
    acceptsSteering: steering.acceptsSteering,
    decisionPending,
    hasActiveQuestion: Boolean(activeQuestionRequest),
  })

  const submitDraftToSteering = async (lane: RunSteeringLane) => {
    const content = draft.trim()
    if (!content) return false
    const stored = await steering.submit(content, lane)
    if (!stored) return false
    setDraft('')
    setCommandOpen(false)
    setMentionOpen(false)
    // Never claim the model acted on it: the Host has only durably accepted it.
    toast.success(
      lane === 'current'
        ? '已接收：将在当前任务的下一个安全边界交给模型'
        : '已排队：保留到下一轮，当前任务不会处理它',
    )
    return true
  }

  const stopRun = () => {
    if (cancelPendingRef.current) return
    if (submitTimer.current !== null) {
      window.clearTimeout(submitTimer.current)
      submitTimer.current = null
    }
    cancelPendingRef.current = true
    void Promise.resolve(onCancel?.()).then((stopped) => {
      if (stopped === false) return
      setStatus('ready')
      onStatusChange?.('ready')
    }).finally(() => {
      cancelPendingRef.current = false
    })
  }

  const sendButton = <PromptInputSubmit
    status={status}
    disabled={(runtimeInitializing || Boolean(inputLockedReason)) && status === 'ready'}
    title={runtimeInitializing ? '正在准备默认专家' : inputLockedReason ?? undefined}
    onStop={stopRun}
    className="fox-send-button"
  >
    {runtimeInitializing ? <LoaderCircle className="animate-spin" size={17} /> : status === 'streaming' ? <CircleStop size={17} /> : <ArrowUp size={18} />}
  </PromptInputSubmit>

  // While the Run is in flight the composer owns two submit lanes and Stop
  // stays a third, separate control. Enter reaches the first `type="submit"`
  // button in the form, which is the steering submit — never Stop, and never a
  // path that starts a second task over the same workspace.
  const steeringControls = <div className="fox-steering-actions" data-testid="fox-steering-actions">
    <Button
      type="submit"
      size="sm"
      className="fox-steering-submit"
      data-testid="fox-steering-submit"
      disabled={steering.busy || !draft.trim()}
      title="加入当前任务：在下一个安全分发边界交给模型，不停止当前任务，不新增工具授权"
    >
      {steering.busy ? <LoaderCircle className="animate-spin" size={13} /> : <CornerDownLeft size={13} />}
      {steeringView.primaryLabel}
    </Button>
    <Button
      type="button"
      size="sm"
      variant="outline"
      className="fox-steering-queue-button"
      data-testid="fox-steering-queue"
      disabled={steering.busy || !draft.trim()}
      title="排队，下一轮处理：保存到队列，当前任务结束后由你发起下一轮时再处理"
      onClick={() => void submitDraftToSteering('next-turn')}
    >
      <Clock3 size={13} />
      {steeringView.secondaryLabel}
    </Button>
    <Button
      type="button"
      size="sm"
      variant="outline"
      className="fox-stop-button"
      data-testid="fox-steering-stop"
      title="停止当前任务（与补充要求互不影响）"
      onClick={stopRun}
    >
      <CircleStop size={15} />
      停止
    </Button>
  </div>

  const sendControls = sendButton

  const expertControl = !(newConversation && entry.draftAgentBackend && entry.draftAgentBackend !== 'fox') && displayExpert && onChangeExpert && <ExpertBindingChip expert={displayExpert} defaultLabel={!activeExpert} directSelection={newConversation} readOnly={expertReadOnly} toolAvailability={expertToolAvailability} onView={activeExpert ? onViewExpert ?? (() => {}) : entry.onViewDefaultExpert ?? (() => {})} onChange={onChangeExpert} />
  const modelControl = isYuxiAgent ? <Models value={model} disabled={!supportsDynamicModelSwitch || chatState === 'running'} onValueChange={setModel} options={modelOptions.map(item => ({ id: item.spec, name: item.displayName || item.modelId, meta: item.providerDisplayName }))} fallbackLabel={defaultModel} /> : <ComposerModelPicker service={modelService} disabled={!runtimeControlled || chatState === 'running' || runtimeInitializing} onOpenSettings={entry.onOpenModelSettings} />

  return (
    <div ref={composerRef} className={`fox-composer-wrap ${centered ? 'is-empty' : ''} ${newConversation ? 'is-new-conversation' : 'is-history'}`}>
      {topSlot}

      <div ref={promptRef} className={`fox-prompt-shell ${decisionPending ? 'is-decision' : ''} ${approvalDecisionPending ? 'is-approval' : ''}`} onFocusCapture={() => setFocused(true)} onBlurCapture={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocused(false) }}>
        {decisionPending ? <div className="fox-decision-card">
          {demoApprovalPending && onApprove && onDeny && <ApprovalPrompt onApprove={onApprove} onDeny={onDeny} />}
          {activeApproval && resolveRuntimeApproval && <div className="fox-decision-approval-list">
            <Suspense fallback={<p role="status">正在读取审批…</p>}><RuntimeApprovalPrompt key={activeApproval.id} approval={activeApproval} onResolve={resolveRuntimeApproval} /></Suspense>
          </div>}
          {pendingApprovals.length === 0 && !demoApprovalPending && demoQuestionPending && onAnswer && <QuestionPrompt onAnswer={onAnswer} />}
          {pendingApprovals.length === 0 && !demoApprovalPending && !demoQuestionPending && activeQuestionRequest && <div className="fox-question-decision-content">
            <header><span><CircleHelp size={15} /></span><div><strong>需要你补充一点信息</strong><small>回答后 Fox 会继续刚才的任务</small></div></header>
            <div className="fox-question-decision-list">
              {activeQuestionRequest.questions.map((question, index) => isConfirmationQuestion(question) ? (
                <Confirmation key={question.questionId} approval={{ id: question.questionId }} state="approval-requested" className="fox-question-confirmation">
                  <ConfirmationRequest>
                    <ConfirmationTitle>{question.question}</ConfirmationTitle>
                    <ConfirmationActions>
                      {question.options.map((option) => <ConfirmationAction key={option.value} disabled={questionSubmitting} variant={(questionSelections[question.questionId] ?? []).includes(option.value) ? 'default' : 'outline'} onClick={() => selectQuestionOption(question, option.value)}>{option.label}</ConfirmationAction>)}
                    </ConfirmationActions>
                  </ConfirmationRequest>
                </Confirmation>
              ) : (
                <section key={question.questionId} className="fox-question-field">
                  <div><b>{activeQuestionRequest.questions.length > 1 ? `${index + 1}. ` : ''}{question.question}</b>{question.multiSelect && <small>可多选</small>}</div>
                  <Suggestions className="fox-question-suggestions">
                    {question.options.map((option) => <Suggestion key={option.value} suggestion={option.value} disabled={questionSubmitting} variant={(questionSelections[question.questionId] ?? []).includes(option.value) ? 'default' : 'outline'} onClick={() => selectQuestionOption(question, option.value)}>{option.label}</Suggestion>)}
                  </Suggestions>
                  {question.allowOther && activeQuestionRequest.questions.length > 1 && <Input disabled={questionSubmitting} value={questionOtherAnswers[question.questionId] ?? ''} onChange={(event) => setQuestionOtherAnswers((current) => ({ ...current, [question.questionId]: event.target.value }))} placeholder="输入这一项的补充答案" />}
                </section>
              ))}
            </div>
            <div className="fox-question-decision-actions">
              {activeQuestionRequest.questions.length === 1 && activeQuestionRequest.questions[0]?.allowOther && <Input disabled={questionSubmitting} value={questionFreeform} onChange={(event) => setQuestionFreeform(event.currentTarget.value)} placeholder="也可以输入自己的答案" />}
              <Button disabled={questionSubmitting} onClick={() => void submitQuestionAnswer()}>{questionSubmitting ? '正在提交…' : '确认并继续'}</Button>
            </div>
          </div>}
          {pendingApprovals.length === 0 && !demoApprovalPending && !demoQuestionPending && !activeQuestionRequest && (goalConfirmationPending || planRevisionPending || workflowGatePending) && goalProgressData && <div className="fox-goal-decision-content">
            <GoalProgress data={goalProgressData} defaultExpanded={false} onEvidenceClick={onEvidenceClick} onResolveConfirmation={onResolveWorkModeConfirmation} onResolvePlanRevision={onResolvePlanRevision} onResolveWorkflowGate={onResolveWorkflowGate} />
          </div>}
        </div> : <ComposerBorderBeam
          enabled={newConversation ?? (centered && !steeringConversationId)}
          active={status === 'streaming' || (centered && focused)}
          brightness={1.5}
          className="fox-prompt-border-beam"
          colorVariant="colorful"
          hueRange={120}
          saturation={1.35}
          size="md"
          strength={1}
          theme={dark ? 'dark' : 'light'}
        >
        <ComposerMenu open={commandOpen}><Command ref={commandMenuRef} value={commandHighlight} onValueChange={setCommandHighlight} loop className={`fox-slash-menu ${commandView === 'knowledge' ? 'is-knowledge' : ''}`}>
          <CommandList>
            {commandView === 'commands' ? <>
              <CommandEmpty>没有匹配的命令</CommandEmpty>
              <CommandGroup>
                {matchingCommands.map(command => <CommandPrimitiveItem asChild key={command.id} value={command.id} onSelect={() => applySlashCommand(command)}><ComposerCommandItem command={{ name: command.id, description: command.title, icon: command.Icon }} active={commandHighlight === command.id} onMouseDown={event => event.preventDefault()} className="w-full rounded-[var(--fox-radius-control)]" /></CommandPrimitiveItem>)}
              </CommandGroup>
            </> : <>
              <CommandEmpty>{runtimeContext.knowledgeLoading ? '正在读取资料库…' : runtimeContext.knowledgeError || '当前没有可使用的资料库'}</CommandEmpty>
              <CommandGroup heading="远程资料库">
                {(runtimeContext.remoteKnowledgeBases ?? []).map((item) => {
                  const reference: KnowledgeReference = { source: 'remote', providerKey: 'yuxi-primary', connectionId: 'yuxi-primary', id: item.id }
                  const selected = selectedKnowledgeReferenceKeys.has(knowledgeReferenceKey(reference))
                  return <CommandItem key={`remote:${item.id}`} value={`远程资料库 ${item.name} ${item.description}`} onSelect={() => { if (!runtimeContext.knowledgeLoading) void runtimeContext.onKnowledgeToggle?.(reference, item.name) }}><span className="fox-command-icon"><Library /></span><span className="fox-command-copy"><strong>{item.name}</strong><small>{item.description || `${item.fileCount} 个文件`}</small></span>{selected && <Check className="fox-command-selected-check" />}</CommandItem>
                })}
              </CommandGroup>
              <div className="fox-slash-knowledge-separator" aria-hidden="true" />
              <CommandGroup heading="本机资料库">
                {(runtimeContext.localKnowledgeBases ?? []).map((item) => {
                  const reference: KnowledgeReference = { source: 'local', providerKey: 'local', id: item.id }
                  const selected = selectedKnowledgeReferenceKeys.has(knowledgeReferenceKey(reference))
                  const availability = localKnowledgePickerState(item)
                  return <CommandItem key={`local:${item.id}`} disabled={!selected && !availability.available} value={`本机资料库 ${item.name} ${item.description ?? ''}`} onSelect={() => { if (!runtimeContext.knowledgeLoading && (selected || availability.available)) void runtimeContext.onKnowledgeToggle?.(reference, item.name) }}><span className="fox-command-icon"><Library /></span><span className="fox-command-copy"><strong>{item.name}</strong><small>{item.description || `${item.documentCount} 个文档`} · {availability.status}</small></span>{selected && <Check className="fox-command-selected-check" />}</CommandItem>
                })}
              </CommandGroup>
            </>}
          </CommandList>
        </Command></ComposerMenu>
        <ComposerMenu open={mentionOpen}><Command ref={mentionMenuRef} value={mentionHighlight} onValueChange={setMentionHighlight} loop className="fox-mention-menu"><div className="fox-command-title"><span>引用项目文件</span><kbd>@</kbd></div><CommandList><CommandEmpty>{mentionChoices.loading ? '正在读取文件…' : mentionChoices.error || (projectRoot ? '没有匹配的项目文件' : '选择项目后可引用文件')}</CommandEmpty><CommandGroup>{mentionFiles.map(person => <CommandPrimitiveItem asChild key={person.name} value={person.name} onSelect={() => applyMention(person.name)}><ComposerPersonItem person={person} active={mentionHighlight === person.name} title={person.name} onMouseDown={event => event.preventDefault()} className="w-full rounded-[var(--fox-radius-control)]" /></CommandPrimitiveItem>)}</CommandGroup></CommandList></Command></ComposerMenu>
        {runtimeControlled && <RunSteeringStrip steering={steering} open={steeringStripOpen} onOpenChange={setSteeringStripOpen} onQueueForNextTask={(content) => setDraft((value) => (value.trim() ? `${value}\n${content}` : content))} />}
        {runtimeControlled && steeringView.retainedForNextTask && steeringView.retainedNotice && (
          <p className="fox-steering-composer-notice" data-testid="fox-steering-retained">
            {steeringView.retainedNotice}
          </p>
        )}
        {runtimeControlled && <ManagedFilesPanel conversationId={steeringConversationId} />}
        {sendNotStarted && <SendNotStartedNotice message={sendNotStarted.message} detail={sendNotStarted.detail} onDismiss={() => setSendNotStarted(null)} />}
        <PromptInput
          accept={supportsImageInput ? 'image/*,.pdf,.txt,.md,.docx,.xls,.xlsx,.ppt,.pptx,.csv,.tsv' : '.pdf,.txt,.md,.docx,.xls,.xlsx,.ppt,.pptx,.csv,.tsv'}
          multiple
          maxFiles={8}
          maxFileSize={MAX_COMPOSER_ATTACHMENT_BYTES}
          onError={({ code, message, detail }) => {
            if (code === 'max_file_size') toast.error('单个附件不能超过 5 MB')
            else if (code === 'max_files') toast.error('一次最多添加 8 个附件')
            else if (code === 'attachment_read_failed') toast.error('附件读取失败，草稿与附件已保留：请删除该附件后重新选择再发送')
            else if (code === 'attachment_send_failed') { toast.error('发送失败，草稿和附件已保留，请重试', { description: message }); setSendNotStarted({ message: message === SEND_NOT_STARTED_MESSAGE ? '发送未开始' : message, detail }) }
            else toast.error('支持 PDF、TXT、MD、DOCX、XLSX、PPTX、CSV、TSV；旧版 DOC、XLS、PPT 请先转换格式')
          }}
          onSubmitStart={({ text, files }) => {
            setSendNotStarted(null)
            if (runtimeInitializing || inputLockedReason) return
            // A supplementary request is not a new turn: it never adds an
            // optimistic user message to the timeline. Its only receipt is the
            // durable steering row the Host stores.
            if (steeringMode) return
            const prompt = text.trim() || (files.length ? '请分析这些附件' : '')
            if (!prompt && !activeQuestionRequest) return
            if (!activeQuestionRequest) flushSync(() => onPromptCommit?.(prompt))
          }}
          onSubmit={async ({ text, files: submittedFiles }) => {
          if (runtimeInitializing) return
          // Read-only conversation (external backend switched off): nothing is
          // sent, so nothing optimistic appears and the draft stays.
          if (inputLockedReason) return false
          if (activeQuestionRequest) {
            if (questionSubmittingRef.current) return
            const answer = buildQuestionAnswer(text)
            if (!answer) {
              toast.error('请先完成需要补充的事项')
              return
            }
            questionSubmittingRef.current = true
            setStatus('streaming')
            const submitted = await submitQuestion?.(answer.text, answer.answers)
            if (!submitted) {
              questionSubmittingRef.current = false
              setStatus('ready')
              return
            }
            setDraft('')
            setQuestionSelections({})
            setQuestionOtherAnswers({})
            onStatusChange?.('streaming')
            return
          }
          if (!text.trim() && submittedFiles.length === 0) return
          if (steeringMode) {
            // The text belongs to the running task, not to a new one. Anything
            // the steering queue cannot carry is refused with a reason instead of
            // being dropped on the floor, and the draft is kept.
            if (submittedFiles.length > 0) {
              toast.error('补充要求只支持文字', {
                description: '附件无法进入运行中任务的补充通道，请等本任务结束后作为新任务发送。',
              })
              return false
            }
            const stored = await submitDraftToSteering('current')
            if (!stored) {
              // The Run may have ended between this view and the Host call; the
              // hook already reconciled, and the draft is still here to resubmit.
              return false
            }
            return true
          }
          const submittedDraft = draft
          const restoreDraft = () => setDraft(submittedDraft)
          setStatus('streaming')
          setDraft('')
          setCommandOpen(false)
          setMentionOpen(false)
          const selectedModel = conversationModelOverride(activeAgent, model, yuxiModels)
          const prompt = text.trim() || '请分析这些附件'
          const runtimeText = selectedSlashCommand ? slashCommandRuntimeText(selectedSlashCommand, prompt) : undefined
          let outcome: Awaited<ReturnType<NonNullable<typeof onSubmitPrompt>>> | undefined
          try {
            outcome = await onSubmitPrompt?.(prompt, selectedModel, submittedFiles, runtimeText)
          } catch (cause) {
            restoreDraft()
            setStatus('ready')
            onStatusChange?.('ready')
            toast.error('发送失败，草稿与附件已保留，请重试', { description: cause instanceof Error ? cause.message : String(cause) })
            return false
          }
          if (outcome === 'error') {
            // The send did not start (for example the attachment could not be
            // saved). Report failure so the composer keeps draft + attachments,
            // with the Host's reason when there is one.
            restoreDraft()
            setStatus('ready')
            onStatusChange?.('ready')
            return { sent: false as const, reason: sendFailureReason?.() ?? null }
          }
          if (outcome === 'question' || outcome === 'approval') {
            // The message was consumed into a question/approval state.
            setStatus('ready')
            onStatusChange?.('ready')
            return true
          }
          setSelectedSlashCommand(null)
          onStatusChange?.('streaming')
          if (runtimeControlled) return
          if (submitTimer.current !== null) window.clearTimeout(submitTimer.current)
          submitTimer.current = window.setTimeout(() => {
            setStatus('ready')
            onStatusChange?.('ready')
            submitTimer.current = null
          }, 3600)
          return true
        }}
          className={`fox-prompt-input ${focused ? 'is-focused' : ''}`}
        >
        <ComposerAttachmentCommandBridge openRef={attachmentDialogRef} />
        <ComposerInitialAttachmentBridge request={initialAttachments} onConsumed={onInitialAttachmentsConsumed} />
        <ComposerAttachmentPreview knowledgeChips={runtimeControlled ? knowledgeChips : []} onKnowledge={openKnowledge} />
        <PromptInputBody>
          <PromptInputTextarea value={draft} disabled={questionSubmitting || Boolean(inputLockedReason)} aria-disabled={inputLockedReason ? true : undefined} title={inputLockedReason ?? undefined} spellCheck={spellcheckEnabled} onChange={onDraftChange} onKeyDown={onComposerKeyDown} className="fox-prompt-textarea" placeholder={inputLockedReason ? '此会话为只读，不能发送' : activeQuestionRequest ? '补充你的答案，或直接选择上方选项' : '给 Fox 发消息，输入 / 查看命令，@ 引用文件…'} />
        </PromptInputBody>
        <PromptInputFooter className="fox-prompt-toolbar">
          <PromptInputTools className="fox-composer-tools">
            <PromptInputActionMenu modal={false}>
              <PromptInputActionMenuTrigger tooltip="添加内容" aria-label="添加内容"><Plus size={18} /></PromptInputActionMenuTrigger>
              <PromptInputActionMenuContent className={`fox-add-menu ${!newConversation ? 'fox-history-add-menu' : ''}`}>
                <PromptInputActionAddAttachments label="添加附件" />
                <PromptInputActionMenuItem onSelect={openKnowledge}><Library />添加资料库</PromptInputActionMenuItem>
                {!newConversation && <div className="fox-history-menu-settings">{expertControl}{modelControl}</div>}
              </PromptInputActionMenuContent>
            </PromptInputActionMenu>
            <PromptInputActionMenu>
              <PromptInputActionMenuTrigger className="fox-execution-control" disabled={!runtimeControlled} tooltip="执行方式（运行中切换，从下一次工具调用起生效）">{projectPermissionMode === 'full_access' ? <ShieldAlert size={14} /> : projectPermissionMode === 'allow' ? <Play size={14} /> : projectPermissionMode === 'read_only' ? <Eye size={14} /> : <CircleHelp size={14} />}<span>{accessModeLabel(projectPermissionMode)}</span></PromptInputActionMenuTrigger>
              <PromptInputActionMenuContent className="fox-add-menu fox-execution-menu">
                {FOX_ACCESS_MODES.map(([value, , label, description]) => (
                  <PromptInputActionMenuItem key={value} title={description} onSelect={() => void onPermissionModeChange?.(value)}>{value === 'full_access' ? <ShieldAlert /> : value === 'allow' ? <Play /> : value === 'read_only' ? <Eye /> : <CircleHelp />}<span>{label}</span>{(projectPermissionMode ?? 'ask') === value && <Check className="fox-menu-check" />}</PromptInputActionMenuItem>
                ))}
              </PromptInputActionMenuContent>
            </PromptInputActionMenu>
            {selectedSlashCommand && <button type="button" className="fox-command-mode-chip" title={`退出${selectedSlashCommand.modeLabel}模式`} onClick={() => setSelectedSlashCommand(null)}><span className="fox-command-mode-icon">{selectedSlashCommand.icon}</span><span>{selectedSlashCommand.modeLabel}</span><span className="fox-command-mode-close"><X size={10} /></span></button>}
            {newConversation && expertControl}
          </PromptInputTools>
          <div className="fox-composer-submit">
            {newConversation && modelControl}
            {steeringMode ? steeringControls : sendControls}
          </div>
        </PromptInputFooter>
        </PromptInput>
        </ComposerBorderBeam>}
        {!decisionPending && newConversation && runtimeControlled && <div className="fox-composer-draft-footer">
          {entry.onSelectProject && entry.onAddProject && <ComposerProjectBar projectRoot={projectRoot} projects={entry.projects} onSelect={entry.onSelectProject} onAdd={entry.onAddProject} />}
          <ComposerAgentPicker value={entry.draftAgentBackend ?? 'fox'} status={entry.agentBackendStatus ?? null} disabled={runtimeInitializing || chatState === 'running'} onSelect={(backend) => entry.onSelectAgentBackend?.(backend)} />
        </div>}
      </div>
    </div>
  )
}

export const MemoizedComposer = memo(Composer)
