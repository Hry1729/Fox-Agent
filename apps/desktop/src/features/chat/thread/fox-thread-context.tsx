import { createContext, useContext } from 'react'
import type { ApprovalDecision, ArtifactRecord, AttachmentRecord, ConversationDetail, KnowledgeBindingRecord, RunEventRecord, ToolCallRecord } from '../../conversations/model/types'
import type { RuntimeSource } from '../timeline/fox-runtime-process-model'

/**
 * Shared facts the thread's message components need that assistant-ui does not
 * carry: the raw run events (the process rows derive live state from them),
 * per-run event grouping, attachment names, knowledge bindings and the host
 * actions (open file / artifact / source). Provided once by the thread root;
 * message parts read it with `useFoxThread`.
 */
export interface FoxThreadContextValue {
  contactChat?: {
    detail: ConversationDetail
    onResolveApproval?: (id: string, decision: ApprovalDecision) => void | Promise<boolean>
    onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>
  }
  /** All run events of the current conversation detail, in seq order. */
  events: RunEventRecord[]
  /** Events grouped by run id, memoized by the provider. */
  eventsByRun: Map<string, RunEventRecord[]>
  toolCalls?: readonly ToolCallRecord[]
  /** Attachment display names by attachment id (tool rows resolve ids). */
  attachmentNames: ReadonlyMap<string, string>
  /** Attachments grouped by message id (user bubbles render their own). */
  attachmentsByMessage: Map<string, AttachmentRecord[]>
  knowledgeBindings: KnowledgeBindingRecord[]
  groupHelpers: typeof import('../runtime-process-groups') | null
  onOpenSource?: (source: RuntimeSource) => void
  onOpenArtifact?: (artifact: ArtifactRecord) => void
  onOpenFileInSidebar?: (path: string) => void
  onRevealFileInExplorer?: (path: string) => void
}

export const FoxThreadContext = createContext<FoxThreadContextValue>({
  events: [],
  eventsByRun: new Map(),
  attachmentNames: new Map(),
  attachmentsByMessage: new Map(),
  knowledgeBindings: [],
  groupHelpers: null,
})

export function useFoxThread() {
  return useContext(FoxThreadContext)
}
