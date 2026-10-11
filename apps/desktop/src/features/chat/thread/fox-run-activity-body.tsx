import { useContext } from 'react'
import type { FoxThreadMessage } from './convert-detail'
import { useFoxThread } from './fox-thread-context'
import { MemoizedRuntimeAssistantMessage } from '../timeline/fox-runtime-assistant-message'
import { TimelineRuntimeContext } from '../fox-shared'

export function FoxRunActivityBody({ message }: { message: FoxThreadMessage }) {
  const fox = useFoxThread()
  const timeline = useContext(TimelineRuntimeContext)
  if (!message.source) return null
  const artifacts = message.content.flatMap(part => part.type === 'data-fox-artifacts' ? part.data : [])
  return <MemoizedRuntimeAssistantMessage message={message.source} kernelMessages={message.source.kernelMessages}
    processEvents={message.runId ? fox.eventsByRun.get(message.runId) ?? [] : []} groupHelpers={fox.groupHelpers}
    singleRunMessage running={message.isActiveRun} runInProgress={message.isActiveRun}
    artifacts={artifacts} assistantName={timeline.assistantName || 'Fox 默认助手'} modelName={timeline.runModel}
    knowledgeBindings={fox.knowledgeBindings} attachmentNames={fox.attachmentNames}
    onOpenSource={fox.onOpenSource} onOpenArtifact={fox.onOpenArtifact} onOpenFileInSidebar={fox.onOpenFileInSidebar}
    onRevealFileInExplorer={fox.onRevealFileInExplorer} failureReason={message.failure ?? undefined} cancelled={message.cancelled} hideFooter />
}
