import { createContext, useContext } from 'react'
import type { ApprovalDecision, ArtifactRecord, AttachmentRecord, ContactBubbleMarkRecord, ContactBubbleRecord, ContactMessageQuoteRecord, ConversationDetail, KnowledgeBindingRecord, RunEventRecord, ToolCallRecord } from '../../conversations/model/types'
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
  /** Committed contact bubbles by run id (C00/K1). */
  contactBubblesByRun?: Map<string, ContactBubbleRecord[]>
  /** Live Kernel preview replies by run id, already carrying the durable
   * `<runId>:kernel:<checkpoint>:response` identity (K1). The run's persisted
   * assistant row renders them; the preview rows themselves stay empty. */
  contactKernelPreviewsByRun?: Map<string, { id: string; text: string; createdAt: number }[]>
  /** C07: committed user bubbles of the loaded window (id == message id). */
  contactUserBubbleIds?: Set<string>
  /** C07: the user's favorites / reactions by durable bubble id. */
  contactBubbleMarks?: Map<string, ContactBubbleMarkRecord>
  /** C07: the quote each loaded message was sent with, by message id. */
  contactMessageQuotes?: Map<string, ContactMessageQuoteRecord>
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
