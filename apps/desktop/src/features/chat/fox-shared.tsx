import { normalizeProjectPermission } from './project-access-dialog-state'
import { type MessageResponseProps } from '@/components/ai-elements/message-response'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { type LocalKnowledgeBaseDto } from '@/features/conversations/api/desktop-client'
import { normalizeAssistantMarkdown } from '@/features/conversations/model/assistant-presentation'
import { type RuntimeQuestion, type RuntimeQuestionRequest } from '@/features/conversations/model/pending-interactions'
import { type ApprovalRecord, type AttachmentRecord, type ChildRunRecord, type ConversationDetail, type ConversationMessage, type GlobalSearchRecord, type KnowledgeBaseRecord, type KnowledgeBindingRecord, type KnowledgeReference, type ProjectRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { type UserProfile } from '@/features/profile/user-profile'
import { Profiler, type ReactNode, Suspense, createContext, lazy, useCallback, useLayoutEffect, useRef } from 'react'
import { type RuntimeSource } from './timeline/fox-runtime-process-model'

const MessageResponse = lazy(() => import('@/components/ai-elements/message-response').then((module) => ({ default: module.MessageResponse })))

export function MarkdownResponse({ children, ...props }: MessageResponseProps) {
  const normalizedChildren = typeof children === 'string' ? normalizeAssistantMarkdown(children) : children
  return (
    <Suspense fallback={<div className="fox-markdown-loading">{typeof children === 'string' ? children : ''}</div>}>
      <MessageResponse {...props}>{normalizedChildren}</MessageResponse>
    </Suspense>
  )
}


export const layoutStorage = {
  sidebarCollapsed: 'fox.layout.sidebarCollapsed',
  sidebarWidth: 'fox.layout.sidebarWidth',
  rightMode: 'fox.layout.rightMode',
  rightTabs: 'fox.layout.rightTabs',
  rightSidebarCollapsed: 'fox.layout.rightSidebarCollapsed',
  rightWidth: 'fox.layout.rightWidth',
  theme: 'fox.theme',
  seenChanges: 'fox.layout.seenChanges'
} as const

// In the profile build, lazily load the telemetry collector and forward React
// Profiler commits to it. The dynamic import sits behind a literal-false flag in
// normal releases, so Rollup DCEs the whole branch and no telemetry chunk or
// runtime cost ships to production.
type RegionRenderHandler = (
  id: string,
  phase: 'mount' | 'update' | 'nested-update',
  actualDuration: number,
  baseDuration: number,
  startTime: number,
  commitTime: number,
) => void
let profilingOnRender: RegionRenderHandler | null = null
if (typeof __FOX_PROFILING__ !== 'undefined' && __FOX_PROFILING__) {
  void import('@/features/profiling/telemetry').then((module) => {
    profilingOnRender = module.profilingCollector.onRender
  })
}

export function recordRegionRender(
  id: string,
  phase: 'mount' | 'update' | 'nested-update',
  actualDuration: number,
  baseDuration: number,
  startTime: number,
  commitTime: number,
) {
  if (typeof performance === 'undefined') return
  if (import.meta.env.DEV) {
    performance.mark(`fox.${id}.${phase}`, { detail: { actualDuration, baseDuration } })
  }
  profilingOnRender?.(id, phase, actualDuration, baseDuration, startTime, commitTime)
}

export function useStableCallback<TArgs extends unknown[], TResult>(callback: (...args: TArgs) => TResult) {
  const callbackRef = useRef(callback)
  useLayoutEffect(() => {
    callbackRef.current = callback
  }, [callback])
  return useCallback((...args: TArgs) => callbackRef.current(...args), [])
}

export const EMPTY_CONVERSATION_MESSAGES: ConversationMessage[] = []
export const EMPTY_CONVERSATION_ATTACHMENTS: AttachmentRecord[] = []
export const EMPTY_RUNTIME_APPROVALS: ApprovalRecord[] = []

export type RightMode = 'activity' | 'changes' | 'browser' | 'files' | 'knowledge' | 'agents'
export interface OpenFileTab {
  id: string
  path: string
  name: string
  artifactId: string | null
}
export interface ChildAgentSelection {
  run: ChildRunRecord
  avatarIndex: number
}
export type ChatState = 'complete' | 'running' | 'question' | 'approval' | 'error' | 'denied'
export type AssistantMode = 'assistant' | 'knowledge'

export const CONVERSATION_FORK_EVENT = 'fox:conversation-fork'

export function requestConversationFork(messageId: string) {
  window.dispatchEvent(new CustomEvent(CONVERSATION_FORK_EVENT, { detail: { messageId } }))
}

export function isConfirmationQuestion(question: RuntimeQuestion) {
  if (question.options.length !== 2 || question.multiSelect) return false
  return question.options.every((option) => /确认|取消|同意|拒绝|继续|停止|是|否|yes|no/i.test(option.label))
}

export async function copyTextWithFeedback(text: string, successMessage: string) {
  try {
    if (!navigator.clipboard?.writeText) throw new Error('Clipboard API unavailable')
    await navigator.clipboard.writeText(text)
    toast.success(successMessage)
  } catch {
    toast.error('复制失败，请重试')
  }
}

export function readStoredBoolean(key: string, fallback: boolean) {
  if (typeof window === 'undefined') return fallback
  const value = window.localStorage.getItem(key)
  return value === null ? fallback : value === '1'
}

export function readDefaultProjectPermission(): ProjectRecord['permissionMode'] {
  return normalizeProjectPermission(
    typeof window === 'undefined' ? null : window.localStorage.getItem('fox.preferences.defaultPermission'),
  )
}

export function readStoredNumber(key: string, fallback: number, min: number, max: number) {
  if (typeof window === 'undefined') return fallback
  const value = Number(window.localStorage.getItem(key))
  return Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback
}

export function readStoredRightMode(): RightMode | null {
  if (typeof window === 'undefined') return 'files'
  const value = window.localStorage.getItem(layoutStorage.rightMode)
  if (value === 'results') return 'files'
  return value === 'activity' || value === 'changes' || value === 'browser' || value === 'files' || value === 'knowledge' || value === 'agents'
    ? value
    : value === 'closed' ? null : 'files'
}

export function readStoredRightTabs(): RightMode[] {
  if (typeof window === 'undefined') return ['files']
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(layoutStorage.rightTabs) ?? 'null')
    if (Array.isArray(value)) {
      return [...new Set(value.filter((item): item is RightMode => (
        item === 'activity' || item === 'changes' || item === 'browser' || item === 'files' || item === 'knowledge' || item === 'agents'
      )))]
    }
  } catch {
    // Fall back to the previously persisted single-page state.
  }
  const mode = readStoredRightMode()
  return mode ? [mode] : []
}

export function IconButton({
  label,
  children,
  onClick,
  active,
  className = '',
  tooltipSide = 'top'
}: {
  label: string
  children: ReactNode
  onClick?: () => void
  active?: boolean
  className?: string
  tooltipSide?: 'top' | 'right' | 'bottom' | 'left'
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={label}
          aria-pressed={active === undefined ? undefined : active}
          className={`fox-icon-button ${active ? 'is-active' : ''} ${className}`}
          onClick={onClick}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent side={tooltipSide}><p>{label}</p></TooltipContent>
    </Tooltip>
  )
}

export function globalSearchKindLabel(kind: GlobalSearchRecord['kind']): string {
  return ({
    conversation: '对话', project: '项目', agent: '专家', digital_colleague: '数字同事',
    knowledge_base: '知识库', knowledge_document: '文档', task: '任务', artifact: '产物',
  })[kind]
}

export type ComposerRuntimeContextValue = {
  mentionFiles?: readonly string[]
  knowledgeBindings: KnowledgeBindingRecord[]
  onKnowledge?: () => void
  onOpenSource?: (source: RuntimeSource) => void
  assistantName?: string
  runModel?: string
  questionRequest?: RuntimeQuestionRequest | null
  onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>
  remoteKnowledgeBases?: KnowledgeBaseRecord[]
  localKnowledgeBases?: LocalKnowledgeBaseDto[]
  knowledgeReferences?: KnowledgeReference[]
  knowledgeReferenceNames?: Record<string, string>
  knowledgeLoading?: boolean
  knowledgeError?: string | null
  onKnowledgeMenuOpen?: () => void
  onKnowledgeToggle?: (reference: KnowledgeReference, name: string) => Promise<boolean>
}

export type TimelineRuntimeContextValue = Pick<ComposerRuntimeContextValue,
  'knowledgeBindings' | 'onOpenSource' | 'assistantName' | 'runModel'
> & {
  recoveryPanel?: ReactNode; userProfile?: UserProfile; authoritativeRunId?: string; authoritativeRunState?: string | null
  deliveryRunId?: string; deliveryRunState?: string | null
  deliveryChecklist?: ConversationDetail['deliveryChecklist']; deliveryChecklistTruncated?: boolean
}

export const ComposerRuntimeContext = createContext<ComposerRuntimeContextValue>({ knowledgeBindings: [] })
export const TimelineRuntimeContext = createContext<TimelineRuntimeContextValue>({ knowledgeBindings: [] })
