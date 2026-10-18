import { createRoot } from 'react-dom/client'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import AssistantUiChat from '../../src/features/chat/thread/fox-assistant-ui-chat'
import '../../src/styles/globals.css'
import '../../src/styles/themes.css'
import '../../src/styles/workbench.css'

const now = Date.now()
const messages = Array.from({ length: 12 }, (_, index) => [
  { id: `u-${index}`, role: 'user', conversationId: 'rail-test', runId: `r-${index}`, content: `第 ${index + 1} 轮：请说明项目方案与验证结果`, status: 'completed', ordinal: index * 2, createdAt: now + index, updatedAt: now + index },
  { id: `process-${index}`, role: 'assistant', conversationId: 'rail-test', runId: `r-${index}`, content: '', status: 'completed', ordinal: index * 2, createdAt: now + index, updatedAt: now + index },
  { id: `a-${index}`, role: 'assistant', conversationId: 'rail-test', runId: `r-${index}`, content: `已完成第 ${index + 1} 轮分析。\n\n这里是方案的具体内容，保留分段以便阅读。\n\n${'检查输入数据、执行步骤以及结果。'.repeat(18)}`, status: 'completed', ordinal: index * 2 + 1, createdAt: now + index, updatedAt: now + index },
]).flat()
const detail = { conversation: { id: 'rail-test' }, lastRun: null, messages, runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [], knowledgeBindings: [], goals: [], tasks: [], evidence: [] }
createRoot(document.getElementById('root')!).render(<TooltipProvider><div className="fox-app fox-shell" style={{ height: '100vh', display: 'flex' }}><div className="fox-content-card" style={{ height: '100%', margin: 0, containerType: 'inline-size', containerName: 'fox-chat' }}><div className="fox-chat-stage" style={{ height: '100%', '--fox-composer-occlusion': '0px' } as any}><div className="fox-thread-shell"><AssistantUiChat conversation={{ enabled: true, detail, streamingText: '' } as any} callbacks={{ onSubmit: () => undefined }} /></div></div></div></div></TooltipProvider>)
