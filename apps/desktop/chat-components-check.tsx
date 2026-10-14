import React, { useState } from 'react'
import { createRoot } from 'react-dom/client'
import './src/styles/globals.css'
import './src/styles/workbench.css'
import './src/styles/workspace-pages.css'
import './src/styles/themes.css'
import './src/features/chat/runtime-process-groups.css'
import './src/features/chat/runtime-process-row.css'
import { RuntimeTimeline } from './src/features/chat/timeline/fox-runtime-timeline'
import { MemoizedComposer } from './src/features/chat/composer/fox-composer'
import { ComposerRuntimeContext } from './src/features/chat/fox-shared'
import { TooltipProvider } from './src/components/ui/tooltip'
import AssistantUiChat from './src/features/chat/thread/fox-assistant-ui-chat'
import { TimelineRuntimeContext } from './src/features/chat/fox-shared'
import { answerDeltaFingerprint } from './src/features/conversations/model/runtime-delta-fingerprint'
import { TurnProcessHeader } from './src/features/chat/turn-process-header'
import { FoxAssistantAvatar } from './src/features/chat/components/FoxAssistantAvatar'
import { MemoizedSidebar } from './src/features/chat/shell/fox-sidebar'
import { PromptInputProvider, usePromptInputController } from './src/components/ai-elements/prompt-input'

const query = new URLSearchParams(location.search)
document.documentElement.dataset.colorTheme = query.get('theme') || 'graphite'
if (query.has('dark')) document.documentElement.classList.add('dark')
document.body.style.margin = '0'
const start = Date.now() - 62000
const event = (seq: number, type: string, data: any = {}) => ({ runId: 'run', seq, eventType: type, event: { type, ...data }, createdAt: start + seq * 1000 })
const initial = '**中文流式输出**\n\n首行  保留两个空格。\n\n'
const blocks = ['新增的一段内容，直接随数据到达显示。\n\n', '```typescript\nconst greeting = "你好，Fox";\nconsole.log(greeting);\n```\n\n', '| 项目 | 结果 |\n| --- | --- |\n| 中文 | 保留 |\n\n最后的回复。']
const agents = [{ id: 'fox', name: 'Fox 默认助手', runtimeType: 'fox', defaultModel: 'configured-model', available: true }, { id: 'colleague', name: '数字同事', runtimeType: 'fox', defaultModel: 'configured-model', available: true }]
function AttachmentSeeds() {
  const {attachments}=usePromptInputController()
  const seeded=React.useRef(false)
  React.useEffect(()=>{if(seeded.current)return;seeded.current=true;attachments.add([new File(['测试附件'], '输入附件示例.txt', {type:'text/plain'}),new File(['列名\n数据'], '销售.csv', {type:'text/csv'})])},[attachments])
  return null
}
function PrefilledComposer({children}:{children:React.ReactNode}) {return <PromptInputProvider><AttachmentSeeds />{children}</PromptInputProvider>}
const ComposerFixture=query.has('attachments')?PrefilledComposer:React.Fragment
function App() {
  const [draft, setDraft] = useState('')
  const [text, setText] = useState(initial)
  const [step, setStep] = useState(0)
  const [running, setRunning] = useState(query.has('live'))
  const [activeAgent, setActiveAgent] = useState(agents[0])
  const [receipt, setReceipt] = useState('界面样例：事件与候选为测试数据。')
  const [processReady, setProcessReady] = useState(!query.has('pending'))
  const [expertSelected,setExpertSelected]=useState(query.has('attachments'))
  const events = [event(1, 'run.started'), event(2, 'reasoning.delta', { delta: '分析当前请求。' }), event(3, 'tool.started', { toolCallId: 'read', tool: 'read', input: { path: 'README.md' } }), event(4, 'tool.completed', { toolCallId: 'read', tool: 'read', result: { content: 'ok' } }), event(5, 'message.delta', { deltaLength: text.length, deltaFingerprint: answerDeltaFingerprint(text) }), ...(running ? [] : [event(6, 'run.completed')])]
  const messages: any[] = query.has('long') ? Array.from({ length: 16 }, (_, i) => ({ id: 'history-' + i, conversationId: 'c', runId: null, role: i % 2 ? 'assistant' : 'user', kind: 'text', content: '历史记录 ' + i + '\n\n' + '用于检查滚动按钮的位置和内容边界。'.repeat(8), status: 'completed', ordinal: i + 1, createdAt: start - 100000, updatedAt: start - 100000 })) : []
  messages.push({ id: 'u', conversationId: 'c', runId: 'run', role: 'user', kind: 'text', content: '请分析并生成中文回复', status: 'completed', ordinal: 18, createdAt: start, updatedAt: start }, { id: 'assistant-run', conversationId: 'c', runId: 'run', role: 'assistant', kind: 'text', content: text, status: running ? 'streaming' : 'completed', ordinal: 19, createdAt: start, updatedAt: start + step * 1000 })
  if (!processReady) events.splice(1)
  return <div style={{ maxWidth: 960, width: '100%', minWidth: 0, flex: 1, height: '100vh', margin: '0 auto', display: 'flex', flexDirection: 'column', padding: '12px 16px', gap: 8 }}>
    <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap', fontSize: 12 }}><span>{receipt}</span>{!processReady && <button onClick={() => { setProcessReady(true); setRunning(true) }}>开始过程</button>}<button onClick={() => { if (step < blocks.length) { setText(t => t + blocks[step]); setStep(step + 1); setRunning(true) } }}>追加文本</button><button onClick={() => setRunning(false)}>结束运行</button></div>
    <div className="fox-chat-stage" style={{ position: 'relative', flex: 1, minHeight: 0 }}>{query.has('empty') ? null : !processReady ? <div className="fox-conversation-content" style={{ padding: '20px 16px' }}><TurnProcessHeader avatar={<FoxAssistantAvatar />} events={events as any} active={running} canToggle={false} collapsed={false} status="正在处理" onToggle={() => {}} /></div> : query.get('engine') === 'aui' ? <div className="fox-thread-shell" style={{ height: '100%' }}><TimelineRuntimeContext.Provider value={{ knowledgeBindings: [], assistantName: 'Fox 默认助手' }}><AssistantUiChat conversation={{ enabled: true, detail: { conversation: { id: 'c', title: '检查', agentId: 'fox', status: 'active' }, messages, runtimeEvents: events, toolCalls: [], approvals: [], attachments: [], artifacts: [], knowledgeBindings: [], expertBindings: [], lastRun: { id: 'run', status: running ? 'running' : 'completed', startedAt: start, finishedAt: running ? null : start + 6000 }, runs: [], hasEarlierMessages: false }, streamingText: running ? text : '', cancel: async () => { setRunning(false); return true }, loadEarlierMessages: async () => {} } as any} callbacks={{ onSubmit: () => {}, onEdit: async () => setReceipt('编辑并重新发送被点击'), onReload: () => setReceipt('重试被点击') }} /></TimelineRuntimeContext.Provider></div> : <RuntimeTimeline messages={messages} attachments={[]} artifacts={[]} events={events as any} runtimeRunning={running} activeRunId={running ? 'run' : undefined} state={running ? 'running' : 'idle'} streamingText={text} onRetry={() => setReceipt('重试被点击')} onRerun={async () => { setReceipt('编辑并重新发送被点击'); return true }} />}</div>
    <ComposerRuntimeContext.Provider value={{ ...React.useContext(ComposerRuntimeContext), localKnowledgeBases: query.has('attachments')?[{id:'kb-preview',name:'项目知识库'} as any]:[], remoteKnowledgeBases: [], knowledgeReferences:query.has('attachments')?[{source:'local',id:'kb-preview'}]:[],mentionFiles: ['docs/测试说明.md', '数据/销售.csv', '报告/周报.docx'] }}>
      <ComposerFixture><MemoizedComposer resetKey={0} draft={draft} setDraft={setDraft} chatState={running ? 'running' : 'idle'} centered={query.has('empty')} newConversation={query.has('empty')} runtimeControlled projectRoot={query.has('attachments')?'D:/projects/aipath-main':undefined} onProject={()=>setReceipt('项目入口被点击')} onKnowledge={()=>setReceipt('知识库入口被点击')} activeExpert={expertSelected?{id:'expert-preview',name:'数据分析专家',available:true} as any:undefined} onViewExpert={()=>setReceipt('专家详情被点击')} onChangeExpert={()=>setReceipt('更换专家被点击')} onRemoveExpert={async()=>{setExpertSelected(false);setReceipt('专家已移除')}} activeAgent={activeAgent as any} agents={agents as any} modelService={{ modelId: 'test-model', name: '测试供应商' } as any} onAgentChange={id => setActiveAgent(agents.find(a => a.id === id)!)} onSubmitPrompt={async prompt => { setReceipt('发送：' + prompt) }} onCancel={() => setRunning(false)} /></ComposerFixture>
    </ComposerRuntimeContext.Provider>
  </div>
}
const sidebarTitles=['我在测试我的本地agent能力与文件审批流程','请检查这个项目的长标题与等待审批状态','已完成的本地文件执行验证与结果整理']
const sidebarConversations=sidebarTitles.flatMap((title,i)=>[null,'D:/projects/aipath-main'].map((projectRoot,j)=>({id:`${j?'project':'sidebar'}-${i}`,title,projectRoot,createdAt:Date.now(),updatedAt:Date.now(),lastMessageAt:Date.now()})))
const sidebarIndicators=new Map(sidebarTitles.flatMap((_,i)=>[`sidebar-${i}`,`project-${i}`].map(id=>[id,['running','approval','completed'][i]])))
const previewSidebar = query.has('sidebar') ? <MemoizedSidebar {...({ collapsed: false, assistantMode: 'assistant', activeView: 'chat', newChatActive: query.has('empty'), activeConversationId: query.has('empty')?undefined:'project-0', runtimeConversations:sidebarConversations,projects:[{id:'preview-project',name:'aipath-main',rootPath:'D:/projects/aipath-main',createdAt:Date.now(),updatedAt:Date.now()}],runIndicators:sidebarIndicators, onModeChange() {}, onNewChat() {}, onNewProjectChat() {}, onAddProject() {}, onOpenConversation() {}, onRenameConversation() {}, onPinConversation() {}, onArchiveConversation() {}, onUnarchiveConversation() {}, onTrashConversation() {}, onRestoreConversation() {}, onPurgeConversation() {}, onDeleteProject() {}, onNavigate() {}, onExitManagement() {}, onExpand() {}, onTheme() {}, onSettings() {} } as any)} /> : null
const previewRoot = createRoot(document.getElementById('root')!)
const innerQuery=new URLSearchParams(query); innerQuery.delete('iframe')
previewRoot.render(query.has('iframe') ? <iframe title="聊天组件独立嵌入预览" src={`./chat-components-check.html?${innerQuery}`} style={{ display:'block',width:'100vw',height:'100vh',border:0 }} /> : <TooltipProvider>{previewSidebar ? <div className="fox-shell" style={{ display: 'flex', height: '100vh', '--fox-sidebar-width':'236px' } as React.CSSProperties}>{previewSidebar}<App /></div> : <App />}</TooltipProvider>)
if (import.meta.hot) import.meta.hot.dispose(() => previewRoot.unmount())
