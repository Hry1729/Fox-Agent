import { useExternalStoreRuntime, type AppendMessage, type CompleteAttachment, type MessageStatus, type PendingAttachment, type ThreadMessageLike } from '@assistant-ui/react'
import { readBlobAsDataUrl } from '@/components/ai-elements/prompt-attachment-data'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { runIsActive, useDesktopConversation } from '@/features/conversations/hooks/use-desktop-conversation'
import type { ConversationMessage } from '../../conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { useMemo, useRef } from 'react'
import { useStableCallback } from '../fox-shared'
import { createFoxThreadCache, deriveFoxThreadMessages, type FoxThreadMessage } from './convert-detail'

type DesktopConversationState = ReturnType<typeof useDesktopConversation>

/**
 * The Fox external-store runtime (migration step 5B). It wraps
 * `useDesktopConversation()` — the single source of truth — into an
 * assistant-ui `AssistantRuntime` via `useExternalStoreRuntime`. Fox keeps
 * owning message boundaries (`joinStrategy: 'none'`), run state and the
 * optimistic-send path; assistant-ui only reads the converted thread.
 */

export interface FoxSubmitFile {
  filename?: string
  mediaType?: string
  url?: string
}

export interface FoxRuntimeCallbacks {
  /** Starts a run with the given prompt and files (existing submit pipeline). */
  onSubmit: (prompt: string, files?: FoxSubmitFile[]) => unknown | Promise<unknown>
  /** Paints the optimistic user message before `onSubmit` resolves. */
  onOptimistic?: (prompt: string) => void
  /** Re-runs from the given user message id with replacement text. */
  onEdit?: (messageId: string, prompt: string) => unknown | Promise<unknown>
  /** Retries the latest run with its original text. */
  onReload?: () => void
}

export interface FoxRuntimeOptions {
  /** The UI-level optimistic user message, if the owner paints one. */
  pendingMessage?: ConversationMessage | null
  /** True while an approval/question/workflow gate is pending. */
  isSendDisabled?: boolean
}

// Attachment rules, mirrored from the legacy composer (PromptInput props):
// at most 8 files, 5 MB each, the same accept list.
const FOX_ATTACHMENT_ACCEPT = 'image/*,.pdf,.txt,.md,.docx,.xls,.xlsx,.ppt,.pptx,.csv,.tsv'
const FOX_ATTACHMENT_MAX_FILES = 8
const FOX_ATTACHMENT_MAX_SIZE = 5 * 1024 * 1024

function promptFromAppendMessage(message: AppendMessage) {
  let prompt = ''
  for (const part of message.content) {
    const textPart = part as { type?: string; text?: string }
    if (textPart?.type === 'text' && typeof textPart.text === 'string') prompt += textPart.text
  }
  return prompt
}

function filesFromAppendMessage(attachments: readonly CompleteAttachment[] | undefined): FoxSubmitFile[] {
  const files: FoxSubmitFile[] = []
  for (const attachment of attachments ?? []) {
    for (const part of attachment.content) {
      const filePart = part as { type?: string; filename?: string; mediaType?: string; url?: string }
      if (filePart.type === 'file' || filePart.type === 'image') {
        files.push({ filename: filePart.filename ?? attachment.name, mediaType: filePart.mediaType, url: filePart.url })
      }
    }
  }
  return files
}

function foxMessageStatus(message: FoxThreadMessage): MessageStatus {
  if (message.isActiveRun) return { type: 'running' }
  if (message.cancelled) return { type: 'incomplete', reason: 'cancelled' }
  if (message.failure) return { type: 'incomplete', reason: 'error', error: { ...message.failure } }
  return { type: 'complete', reason: 'stop' }
}

/** The `useExternalMessageConverter` callback: FoxThreadMessage → ThreadMessageLike. */
function convertFoxThreadMessage(message: FoxThreadMessage): ThreadMessageLike {
  return {
    role: message.role,
    id: message.id,
    createdAt: new Date(message.createdAt),
    content: message.content,
    // This assistant-ui release rejects `status` on user messages
    // ("status is only supported for assistant messages").
    status: message.role === 'assistant' ? foxMessageStatus(message) : undefined,
    attachments: message.attachments.length ? message.attachments : undefined,
    metadata: { custom: { fox: message }, ...(message.optimistic ? { isOptimistic: true } : {}) },
  }
}

export function useFoxRuntime(
  conversation: DesktopConversationState,
  callbacks: FoxRuntimeCallbacks,
  options: FoxRuntimeOptions = {},
) {
  const detail = conversation.detail
  // The conversion cache must outlive renders so unchanged messages keep their
  // object identity; `deriveFoxThreadMessages` reuses entries keyed by message
  // id and revision, so streaming re-converts only the running message.
  const cacheRef = useRef(createFoxThreadCache())
  // Module-constant converter: unchanged messages keep their object identity,
  // which is what the runtime's internal conversion cache keys on.
  const convert = useRef(convertFoxThreadMessage).current

  const isRunning = conversation.enabled && runIsActive(detail)
  const messages = useMemo(
    () => deriveFoxThreadMessages({
      detail: conversation.enabled ? detail : null,
      pendingMessage: options.pendingMessage,
      streamingText: isRunning ? conversation.streamingText : '',
    }, cacheRef.current),
    [conversation.enabled, conversation.streamingText, detail, isRunning, options.pendingMessage],
  )

  const onNew = useStableCallback(async (message: AppendMessage) => {
    const prompt = promptFromAppendMessage(message)
    if (!prompt.trim()) return
    const files = filesFromAppendMessage(message.attachments)
    // Reuse the legacy optimistic path: the user message paints immediately,
    // before the (async) conversation creation and send round-trip.
    callbacks.onOptimistic?.(prompt)
    await callbacks.onSubmit(prompt, files.length ? files : undefined)
  })

  const onEdit = useStableCallback(async (message: AppendMessage) => {
    const prompt = promptFromAppendMessage(message)
    if (!prompt.trim()) return
    // Fox only allows editing the last user message; resolve its id the same
    // way `retryLatestRun` does instead of trusting the runtime branch parent.
    const lastUser = detail ? [...detail.messages].reverse().find((item) => item.role === 'user') : null
    if (!lastUser) return
    await callbacks.onEdit?.(lastUser.id, prompt)
  })

  const onReload = useStableCallback(async () => {
    callbacks.onReload?.()
  })

  const onCancel = useStableCallback(async () => {
    await conversation.cancel()
  })

  const onLoadEarlier = useStableCallback(async () => {
    await conversation.loadEarlierMessages()
  })

  const feedback = useMemo(() => ({
    submit: ({ message, type }: { message: { id: string }; type: 'positive' | 'negative' }) => {
      void desktopClient.saveMessageFeedback(message.id, type)
        .catch((cause) => toast.error(cause instanceof Error ? cause.message : String(cause)))
    },
  }), [])

  const attachmentCountRef = useRef(0)
  const attachments = useMemo(() => ({
    accept: FOX_ATTACHMENT_ACCEPT,
    add: async ({ file }: { file: File }): Promise<PendingAttachment> => {
      if (file.size > FOX_ATTACHMENT_MAX_SIZE) throw new Error('单个附件不能超过 5 MB')
      if (attachmentCountRef.current >= FOX_ATTACHMENT_MAX_FILES) throw new Error('一次最多添加 8 个附件')
      const mediaType = file.type || 'application/octet-stream'
      attachmentCountRef.current += 1
      return {
        id: crypto.randomUUID(),
        type: mediaType.startsWith('image/') ? 'image' : mediaType === 'application/pdf' ? 'document' : 'file',
        name: file.name,
        file,
        contentType: mediaType,
        status: { type: 'running', reason: 'uploading', progress: 1 },
      }
    },
    remove: async () => {
      attachmentCountRef.current = Math.max(0, attachmentCountRef.current - 1)
    },
    send: async (attachment: PendingAttachment): Promise<CompleteAttachment> => {
      // Same read path as the legacy composer: a data: URL the host saves
      // inside `send()` before anything optimistic renders.
      const url = await readBlobAsDataUrl(attachment.file)
      return {
        ...attachment,
        status: { type: 'complete' },
        content: [{ type: 'file', filename: attachment.name, mimeType: attachment.contentType ?? 'application/octet-stream', data: url, sourceType: 'url' }],
      }
    },
  }), [])

  // `useStableCallback` handlers read through refs, so their identity (and the
  // runtime built from it) never changes when the owner re-renders.
  return useExternalStoreRuntime({
    convertMessage: convert,
    messages,
    isRunning,
    isSendDisabled: options.isSendDisabled,
    hasEarlier: Boolean(detail?.hasEarlierMessages),
    onNew,
    onEdit,
    onReload,
    onCancel,
    onLoadEarlier,
    adapters: { feedback, attachments },
  })
}
