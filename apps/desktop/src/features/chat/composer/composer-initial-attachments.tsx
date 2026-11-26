import { useEffect, useLayoutEffect, useRef } from 'react'
import { usePromptInputAttachments } from '@/components/ai-elements/prompt-input'

export interface ComposerInitialAttachments {
  requestId: number
  files: File[]
  restoreDraft?: true
}

export function ComposerInitialAttachmentBridge({ request, onConsumed, onFilesChange }: { request?: ComposerInitialAttachments | null; onConsumed?: (requestId: number) => void; onFilesChange?: (files: File[]) => void }) {
  const attachments = usePromptInputAttachments()
  const consumed = useRef<number | null>(null)
  useLayoutEffect(() => {
    if (request && consumed.current !== request.requestId) return
    onFilesChange?.(attachments.files.flatMap(item => item.file ? [item.file] : []))
  }, [attachments.files, onFilesChange, request])
  useEffect(() => {
    if (!request || consumed.current === request.requestId) return
    consumed.current = request.requestId
    const files = request.restoreDraft
      ? request.files.filter(file => !attachments.files.some(attachment => attachment.file === file))
      : request.files
    attachments.add(files)
    onConsumed?.(request.requestId)
  }, [attachments.add, attachments.files, onConsumed, request])
  return null
}
