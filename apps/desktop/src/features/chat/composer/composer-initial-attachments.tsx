import { useEffect, useRef } from 'react'
import { usePromptInputAttachments } from '@/components/ai-elements/prompt-input'

export interface ComposerInitialAttachments {
  requestId: number
  files: File[]
}

export function ComposerInitialAttachmentBridge({ request, onConsumed }: { request?: ComposerInitialAttachments | null; onConsumed?: (requestId: number) => void }) {
  const attachments = usePromptInputAttachments()
  const consumed = useRef<number | null>(null)
  useEffect(() => {
    if (!request || consumed.current === request.requestId) return
    consumed.current = request.requestId
    attachments.add(request.files)
    onConsumed?.(request.requestId)
  }, [attachments.add, onConsumed, request])
  return null
}
