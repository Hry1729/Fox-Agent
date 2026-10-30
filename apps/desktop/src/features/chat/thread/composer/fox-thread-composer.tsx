import { ComposerPrimitive, useAui, useAuiState } from '@assistant-ui/react'
import { ExpertBindingChip } from '../../components/ExpertBindingChip'
import { type ExpertToolAvailability } from '../../components/expert-binding-ui'
import { ManagedFilesPanel } from '../../components/ManagedFilesPanel'
import { RunSteeringStrip } from '../../components/RunSteeringStrip'
import { steeringComposerView, steeringSubmitMode } from '../../components/steering-presentation'
import { useRunSteering } from '../../hooks/use-run-steering'
import { localKnowledgePickerState } from '../../knowledge-picker-state'
import { ComposerRuntimeContext, IconButton } from '../../fox-shared'
import { type SlashCommand, slashCommandRuntimeText, slashCommands } from '../../composer/fox-slash-commands'
import { insertPromptNewline, readPromptSendShortcut, resolvePromptEnterAction } from '@/components/ai-elements/prompt-send-shortcut'
import { Button } from '@/components/ui/button'
import { Command, CommandEmpty, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { knowledgeReferenceFromLegacyBinding, knowledgeReferenceKey, type RunSteeringLane } from '@/features/conversations/api/desktop-client'
import { type KnowledgeReference, type ProjectRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { ArrowUp, Check, ChevronDown, CircleStop, Clock3, CornerDownLeft, FileText, FolderOpen, Library, LoaderCircle, Plus, Terminal, X } from 'lucide-react'
import { type ChangeEvent, type FormEvent, type KeyboardEvent, type ReactNode, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { FoxDecisionCard, FOX_PERMISSION_MODES } from './fox-decision-card'
import type { ComposerAccessMode } from '../../access-mode'

/**
 * The assistant-ui composer (migration step 5D). Built on
 * `ComposerPrimitive.Root/Input/Send/Cancel`; the runtime owns the text, the
 * attachments and the send path (onNew → conversation.send), while Fox owns the
 * slash commands, the permission mode, the decision cards, the steering lanes
 * and the draft. Keyboard rules come from `prompt-send-shortcut.ts` and follow
 * the 发送方式 preference: Enter sends (Shift/Ctrl/Cmd+Enter break the line), or
 * Ctrl/Cmd+Enter sends (Enter breaks the line). An IME-owned Enter never sends.
 */
export function FoxThreadComposer({ runtimeControlled, runtimeInitializing = false, projectRoot, projectPermissionMode, expertReadOnly = false, expertToolAvailability, activeExpert, supportsImageInput = false, draft, setDraft, resetKey, steeringConversationId, steeringActive = false, questionRequest, onSubmitQuestion, runtimeApprovals, onResolveApproval, goalProgressData, onResolveWorkModeConfirmation, onResolvePlanRevision, onResolveWorkflowGate, onEvidenceClick, onProject, onPermissionModeChange, onKnowledge, onViewExpert, onChangeExpert, onRemoveExpert, onHeightChange, onCancel }: {
  runtimeControlled: boolean
  runtimeInitializing?: boolean
  projectRoot?: string | null
  projectPermissionMode?: ComposerAccessMode | null
  expertReadOnly?: boolean
  expertToolAvailability?: ExpertToolAvailability
  activeExpert?: Parameters<typeof ExpertBindingChip>[0]['expert'] | null
  supportsImageInput?: boolean
  /** The persisted draft (workbench state); restored when the composer mounts or resets. */
  draft: string
  setDraft: (value: string) => void
  resetKey: number
  steeringConversationId?: string
  steeringActive?: boolean
  questionRequest?: Parameters<typeof FoxDecisionCard>[0]['questionRequest']
  onSubmitQuestion?: Parameters<typeof FoxDecisionCard>[0]['onSubmitQuestion']
  runtimeApprovals?: Parameters<typeof FoxDecisionCard>[0]['runtimeApprovals']
  onResolveApproval?: Parameters<typeof FoxDecisionCard>[0]['onResolveApproval']
  goalProgressData?: Parameters<typeof FoxDecisionCard>[0]['goalProgressData']
  onResolveWorkModeConfirmation?: Parameters<typeof FoxDecisionCard>[0]['onResolveWorkModeConfirmation']
  onResolvePlanRevision?: Parameters<typeof FoxDecisionCard>[0]['onResolvePlanRevision']
  onResolveWorkflowGate?: Parameters<typeof FoxDecisionCard>[0]['onResolveWorkflowGate']
  onEvidenceClick?: Parameters<typeof FoxDecisionCard>[0]['onEvidenceClick']
  onProject?: () => void
  onPermissionModeChange?: (permissionMode: ComposerAccessMode) => void | Promise<void>
  onKnowledge?: () => void
  onViewExpert?: () => void
  onChangeExpert?: () => void
  onRemoveExpert?: () => void | Promise<void>
  onHeightChange?: (height: number) => void
  onCancel?: () => void | Promise<unknown>
}) {
  const aui = useAui()
  const runtimeContext = useContext(ComposerRuntimeContext)
  const composerText = useAuiState((state) => state.composer.text)
  const attachments = useAuiState((state) => state.composer.attachments)
  const canSend = useAuiState((state) => state.composer.canSend)
  const threadIsRunning = useAuiState((state) => state.thread.isRunning)
  const isRunning = runtimeControlled && threadIsRunning

  // --- draft (workbench owns persistence; the runtime owns the text) ---------
  const pushedTextRef = useRef('')
  useEffect(() => {
    if (composerText === pushedTextRef.current) return
    pushedTextRef.current = composerText
    setDraft(composerText)
  }, [composerText, setDraft])
  // Restore the draft when the composer mounts or the owner resets it. Never
  // write back a value the composer itself just pushed.
  useEffect(() => {
    if (draft === pushedTextRef.current) return
    pushedTextRef.current = draft
    void aui.composer.setText(draft)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resetKey])

  // --- slash commands --------------------------------------------------------
  const [commandOpen, setCommandOpen] = useState(false)
  const [commandView, setCommandView] = useState<'commands' | 'knowledge'>('commands')
  const [selectedSlashCommand, setSelectedSlashCommand] = useState<SlashCommand | null>(null)
  const enabledKnowledgeBindings = (runtimeContext.knowledgeBindings ?? []).filter((binding) => binding.enabled !== false)
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

  const onInputChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    const value = event.currentTarget.value
    const leadingValue = value.trimStart()
    const hasSlashCommand = /^\/[^\s]*$/u.test(leadingValue)
    setCommandOpen(hasSlashCommand)
    if (hasSlashCommand) setCommandView('commands')
  }

  const applySlashCommand = (command: SlashCommand) => {
    const stripped = composerText.replace(/^\s*\/[^\s]*/u, '').trimStart()
    pushedTextRef.current = stripped
    void aui.composer.setText(stripped)
    if (command.action === 'attachments') {
      setCommandOpen(false)
      setCommandView('commands')
      setSelectedSlashCommand(null)
      fileInputRef.current?.click()
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

  // --- attachments -----------------------------------------------------------
  const fileInputRef = useRef<HTMLInputElement | null>(null)
  const addFiles = async (files: FileList | null) => {
    if (!files?.length) return
    for (const file of Array.from(files)) {
      try {
        await aui.composer.addAttachment(file)
      } catch (cause) {
        toast.error(cause instanceof Error ? cause.message : '附件添加失败')
      }
    }
  }
  // Captured during render: the aui client proxy rejects property access from
  // event handlers, but the runtime itself is a plain object.
  const composerRuntime = aui.composer.__internal_getRuntime?.()
  const removeAttachment = (id: string) => {
    if (!composerRuntime) return
    const index = composerRuntime.getState().attachments.findIndex((attachment) => attachment.id === id)
    if (index < 0) return
    void composerRuntime.getAttachmentByIndex(index).remove()
  }

  // --- send / steering -------------------------------------------------------
  const [steeringStripOpen, setSteeringStripOpen] = useState(false)
  const steering = useRunSteering({
    conversationId: steeringConversationId,
    active: runtimeControlled && (steeringActive || isRunning),
    enabled: runtimeControlled,
  })
  const steeringView = useMemo(
    () => steeringComposerView({
      active: runtimeControlled && (steeringActive || isRunning),
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
    [runtimeControlled, steering.acceptsSteering, steering.rows, steering.runId, steering.runState, steeringActive, isRunning],
  )
  const decisionPending = Boolean(questionRequest)
    || (runtimeApprovals ?? []).some((approval) => approval.status === 'pending')
    || (Boolean(onResolveWorkModeConfirmation) && goalProgressData?.goal.status === 'proposed')
    || (Boolean(onResolvePlanRevision) && Boolean(goalProgressData?.planRevisions.some((plan) => plan.status === 'proposed')))
    || (Boolean(onResolveWorkflowGate) && Boolean(goalProgressData?.expertWorkflow?.gates.some((gate) => gate.status === 'pending')))
  const steeringMode = steeringSubmitMode({
    runtimeControlled,
    status: isRunning ? 'streaming' : 'ready',
    acceptsSteering: steering.acceptsSteering,
    decisionPending,
    hasActiveQuestion: Boolean(questionRequest),
  })

  const submitSteering = async (lane: RunSteeringLane) => {
    const content = composerText.trim()
    if (!content) return false
    const stored = await steering.submit(content, lane)
    if (!stored) return false
    pushedTextRef.current = ''
    void aui.composer.setText('')
    setCommandOpen(false)
    toast.success(
      lane === 'current'
        ? '已接收：将在当前任务的下一个安全边界交给模型'
        : '已排队：保留到下一轮，当前任务不会处理它',
    )
    return true
  }

  const submit = () => {
    if (!canSend || runtimeInitializing) return
    if (selectedSlashCommand) {
      const rewritten = slashCommandRuntimeText(selectedSlashCommand, composerText.trim())
      pushedTextRef.current = ''
      void aui.composer.setText(rewritten)
      setSelectedSlashCommand(null)
      setCommandOpen(false)
      // The runtime re-reads its state inside send(), so the rewrite lands.
      setTimeout(() => aui.composer.send(), 0)
      return
    }
    aui.composer.send()
  }

  const onInputKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    const action = resolvePromptEnterAction(event.nativeEvent, readPromptSendShortcut())
    if (action === 'send' && steeringMode) {
      // While a task runs and accepts steering, the form's default is the
      // current-task supplement; the primitive's internal Enter submit must
      // never reach it (the primitive itself also stands down while running).
      event.preventDefault()
      void submitSteering('current')
      return
    }
    if (action === 'newline') {
      // Fox breaks the line itself (Ctrl/Cmd+Enter and Shift+Enter): the
      // primitive would send on Ctrl/Cmd+Enter, and a controlled textarea
      // cannot rely on the browser's default insertion.
      event.preventDefault()
      const el = event.currentTarget
      const next = insertPromptNewline(el.value, el.selectionStart, el.selectionEnd)
      void aui.composer.setText(next.value)
      requestAnimationFrame(() => {
        el.setSelectionRange(next.caret, next.caret)
      })
      return
    }
    if (action === 'default' && event.key === 'Enter') {
      // Alt+Enter (and an IME-owned Enter): browser default, never the
      // primitive's submit.
      event.preventDefault()
      return
    }
    if (event.key === 'Escape') {
      setCommandOpen(false)
      setCommandView('commands')
    }
    // 'send' falls through to the primitive's own Enter handling (a plain Enter
    // without steering mode); every other case was resolved above.
  }

  const onFormSubmit = (event: FormEvent) => {
    if (!selectedSlashCommand) return
    // Rewrite the prompt for the active slash mode, then send through the
    // runtime ourselves (the internal send would use the unrewritten text).
    event.preventDefault()
    submit()
  }

  // --- height measurement (the stage layout depends on it) --------------------
  const composerRef = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    const composer = composerRef.current
    const stage = composer?.closest<HTMLElement>('.fox-chat-stage')
    if (!composer || !stage || !onHeightChange) return
    let animationFrame = 0
    const updateHeight = () => {
      window.cancelAnimationFrame(animationFrame)
      animationFrame = window.requestAnimationFrame(() => {
        const stageBottom = stage.getBoundingClientRect().bottom
        const composerTop = composer.getBoundingClientRect().top
        onHeightChange(Math.ceil(Math.max(0, stageBottom - composerTop + 6)))
      })
    }
    updateHeight()
    const observer = new ResizeObserver(updateHeight)
    observer.observe(stage)
    observer.observe(composer)
    window.addEventListener('resize', updateHeight)
    return () => {
      window.cancelAnimationFrame(animationFrame)
      observer.disconnect()
      window.removeEventListener('resize', updateHeight)
    }
  }, [onHeightChange])

  useEffect(() => {
    setCommandOpen(false)
    setCommandView('commands')
    setSelectedSlashCommand(null)
    setSteeringStripOpen(false)
  }, [resetKey])

  const spellcheckEnabled = typeof window === 'undefined' || window.localStorage.getItem('fox.preferences.spellcheck') !== '0'
  const accept = supportsImageInput ? 'image/*,.pdf,.txt,.md,.docx,.xls,.xlsx,.ppt,.pptx,.csv,.tsv' : '.pdf,.txt,.md,.docx,.xls,.xlsx,.ppt,.pptx,.csv,.tsv'

  const contextRow: ReactNode[] = []
  if (runtimeControlled && projectRoot) {
    contextRow.push(<button type="button" key="project" className="fox-composer-context-chip is-authorized" onClick={onProject} title={projectRoot}><FolderOpen size={13} /><span>{projectRoot.split(/[\\/]/).filter(Boolean).at(-1)}</span><Check size={12} /></button>)
  }
  knowledgeChips.forEach((chip) => {
    contextRow.push(<button type="button" key={chip.key} className="fox-composer-context-chip is-authorized is-knowledge" onClick={openKnowledge} title={chip.name}><Library size={13} /><span>{chip.name}</span><Check size={12} /></button>)
  })
  attachments.forEach((attachment) => {
    contextRow.push(<button type="button" key={attachment.id} className="fox-composer-context-chip is-attachment" title={attachment.name} onClick={() => removeAttachment(attachment.id)}><FileText size={13} /><span>{attachment.name}</span><X size={11} /></button>)
  })

  const steeringControls = <div className="fox-steering-actions" data-testid="fox-steering-actions">
    <Button
      type="button"
      size="sm"
      className="fox-steering-submit"
      data-testid="fox-steering-submit"
      disabled={steering.busy || !composerText.trim()}
      title="加入当前任务：在下一个安全分发边界交给模型，不停止当前任务，不新增工具授权"
      onClick={() => void submitSteering('current')}
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
      disabled={steering.busy || !composerText.trim()}
      title="排队，下一轮处理：保存到队列，当前任务结束后由你发起下一轮时再处理"
      onClick={() => void submitSteering('next-turn')}
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
      onClick={() => void onCancel?.()}
    >
      <CircleStop size={15} />
      停止
    </Button>
  </div>

  return (
    <div ref={composerRef} className="fox-thread-composer-wrap">
      <FoxDecisionCard
        questionRequest={questionRequest}
        onSubmitQuestion={onSubmitQuestion}
        runtimeApprovals={runtimeApprovals}
        onResolveApproval={onResolveApproval}
        goalProgressData={goalProgressData}
        onResolveWorkModeConfirmation={onResolveWorkModeConfirmation}
        onResolvePlanRevision={onResolvePlanRevision}
        onResolveWorkflowGate={onResolveWorkflowGate}
        onEvidenceClick={onEvidenceClick}
      />
      {runtimeControlled && steeringView.retainedForNextTask && steeringView.retainedNotice && (
        <p className="fox-steering-composer-notice" data-testid="fox-steering-retained">
          {steeringView.retainedNotice}
        </p>
      )}
      {runtimeControlled && <RunSteeringStrip steering={steering} open={steeringStripOpen} onOpenChange={setSteeringStripOpen} onQueueForNextTask={(content) => {
        const next = composerText.trim() ? `${composerText}\n${content}` : content
        pushedTextRef.current = next
        void aui.composer.setText(next)
      }} />}
      {runtimeControlled && <ManagedFilesPanel conversationId={steeringConversationId} />}
      {commandOpen && <Command loop className={`fox-slash-menu ${commandView === 'knowledge' ? 'is-knowledge' : ''}`}>
        <CommandList>
          {commandView === 'commands' ? <>
            <CommandEmpty>没有匹配的命令</CommandEmpty>
            <CommandGroup>
              {slashCommands.map((command) => <CommandItem key={command.id} value={command.id} onSelect={() => applySlashCommand(command)}><span className="fox-command-icon">{command.icon}</span><span className="fox-command-copy"><strong>/{command.id}</strong><small>{command.title} · {command.detail}</small></span></CommandItem>)}
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
      </Command>}
      <ComposerPrimitive.Root className={`fox-thread-composer-form ${isRunning ? 'is-running' : ''}`} onSubmit={onFormSubmit}>
        <input ref={fileInputRef} type="file" accept={accept} multiple hidden onChange={(event) => { void addFiles(event.target.files); event.target.value = '' }} />
        {contextRow.length > 0 && <div className="fox-thread-composer-context">{contextRow}</div>}
        <ComposerPrimitive.Input
          className="fox-thread-composer-input"
          placeholder={questionRequest ? '补充你的答案，或直接选择上方选项' : '给 Fox 发消息，输入 / 查看命令…'}
          rows={1}
          autoFocus
          spellCheck={spellcheckEnabled}
          cancelOnEscape={false}
          onChange={onInputChange}
          onKeyDown={onInputKeyDown}
        />
        <div className="fox-thread-composer-toolbar">
          <div className="fox-thread-composer-tools">
            <IconButton label="添加文件" onClick={() => fileInputRef.current?.click()}><Plus size={16} /></IconButton>
            <DropdownMenu>
              <Tooltip>
                <TooltipTrigger asChild>
                  <DropdownMenuTrigger asChild>
                    <Button variant="ghost" size="sm" className="fox-execution-control" disabled={!runtimeControlled} aria-label="执行方式">
                      <Terminal size={14} />
                      <span>{(FOX_PERMISSION_MODES.find(([value]) => value === (projectPermissionMode ?? 'ask')) ?? FOX_PERMISSION_MODES[0])[1]}</span>
                      <ChevronDown size={13} />
                    </Button>
                  </DropdownMenuTrigger>
                </TooltipTrigger>
                <TooltipContent side="top"><p>执行方式（运行中切换，从下一次工具调用起生效）</p></TooltipContent>
              </Tooltip>
              <DropdownMenuContent className="fox-execution-menu" side="top" align="start">
                {FOX_PERMISSION_MODES.map(([value, label]) => (
                  <DropdownMenuItem key={value} onSelect={() => void onPermissionModeChange?.(value)}><Terminal /><span>{label}</span>{(projectPermissionMode ?? 'ask') === value && <Check className="fox-menu-check" />}</DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
            {selectedSlashCommand && <button type="button" className="fox-command-mode-chip" title={`退出${selectedSlashCommand.modeLabel}模式`} onClick={() => setSelectedSlashCommand(null)}><span className="fox-command-mode-icon">{selectedSlashCommand.icon}</span><span>{selectedSlashCommand.modeLabel}</span><span className="fox-command-mode-close"><X size={10} /></span></button>}
            {runtimeControlled && activeExpert && onViewExpert && onChangeExpert && onRemoveExpert && <ExpertBindingChip expert={activeExpert} readOnly={expertReadOnly} toolAvailability={expertToolAvailability} onView={onViewExpert} onChange={onChangeExpert} onRemove={onRemoveExpert} />}
          </div>
          <div className="fox-thread-composer-submit">
            {steeringMode ? steeringControls
              : isRunning
                ? <ComposerPrimitive.Cancel asChild>
                    <Button type="button" size="icon" className="fox-thread-stop-button" aria-label="停止" title="停止任务"><CircleStop size={16} /></Button>
                  </ComposerPrimitive.Cancel>
                : <Tooltip>
                    <TooltipTrigger asChild>
                      <button type="submit" className="fox-thread-send-button" disabled={!canSend || runtimeInitializing} aria-label="发送" title={runtimeInitializing ? '正在准备默认专家' : '发送'}>
                        {runtimeInitializing ? <LoaderCircle className="animate-spin" size={16} /> : <ArrowUp size={17} />}
                      </button>
                    </TooltipTrigger>
                    <TooltipContent side="top"><p>发送</p></TooltipContent>
                  </Tooltip>}
          </div>
        </div>
      </ComposerPrimitive.Root>
    </div>
  )
}
