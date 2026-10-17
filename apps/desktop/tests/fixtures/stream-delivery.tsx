import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import AssistantUiChat from '../../src/features/chat/thread/fox-assistant-ui-chat'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'

const now = Date.now()
const initial: any = {
  conversation: { id: 'stream-test' }, lastRun: { id: 'run-test', status: 'running', startedAt: now },
  messages: [{ id: 'u', role: 'user', conversationId: 'stream-test', runId: null, content: '请分析并生成报告', status: 'completed', ordinal: 0, createdAt: now, updatedAt: now }],
  runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [], knowledgeBindings: [], goals: [], tasks: [], evidence: [],
}
function Fixture() {
  const [detail, setDetail] = useState(initial)
  const [streamingText, setText] = useState('')
  ;(window as any).streamFixture = {
    detail, setDetail, setText,
    finish(text: string, artifacts: unknown[] = []) {
      setText('')
      setDetail({ ...detail, lastRun: { ...detail.lastRun, status: 'completed', finishedAt: Date.now() }, artifacts,
        messages: [...initial.messages, { id: 'assistant-run-test', role: 'assistant', conversationId: 'stream-test', runId: 'run-test', content: text, status: 'completed', ordinal: 1, createdAt: now, updatedAt: now }] })
    },
  }
  return <main style={{ height: '100vh', width: '100%', background: 'var(--background)' }}>
    <div className="fox-chat-stage" style={{ height: '100%', '--fox-composer-occlusion': '20px' } as any}>
      <div className="fox-thread-shell"><AssistantUiChat conversation={{ enabled: true, detail, streamingText } as any} callbacks={{ onSubmit: () => undefined }} onOpenArtifact={artifact => { (window as any).openedArtifact = artifact.id }} /></div>
    </div>
  </main>
}
createRoot(document.getElementById('root')!).render(<Fixture />)
