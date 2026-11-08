import { useMemo, useState, useEffect } from 'react'
import { AssistantRuntimeProvider, ThreadPrimitive, useExternalStoreRuntime, type ThreadMessageLike } from '@assistant-ui/react'
import { MemoizedComposer } from '../src/features/chat/composer/fox-composer'
import { ComposerEntryContext } from '../src/features/chat/composer/composer-entry-context'
import { ComposerRuntimeContext } from '../src/features/chat/fox-shared'
import { RuntimeApprovalPrompt as CurrentApproval } from '../src/features/chat/components/RuntimeApprovalPrompt'
import { RuntimeApprovalPrompt as ProposedApproval } from './approval-preview'
import { FoxUserMessage } from '../src/features/chat/thread/fox-user-message'
import { FoxAssistantMessage } from '../src/features/chat/thread/fox-assistant-message'
import { Button } from '../src/components/ui/button'
import { TooltipProvider } from '../src/components/ui/tooltip'
import { desktopClient } from '../src/features/conversations/api/desktop-client'
import type { AgentBackendId, AgentBackendStatus } from '../src/features/conversations/model/agent-backend'
import type { ApprovalRecord, ProjectRecord } from '../src/features/conversations/model/types'
import '../src/features/chat/thread/fox-thread.css'

// 独立预览只替换持久化边界。组件仍执行原来的提交、禁用和回执逻辑。
desktopClient.approvalAllowProjectRule = async (_id, prefix) => {
  window.dispatchEvent(new CustomEvent('review-result', { detail: `演示：已选择项目规则 ${prefix}；没有写入权限。` }))
  return true
}
desktopClient.saveMessageFeedback = async (messageId, sentiment, category, comment) => {
  window.dispatchEvent(new CustomEvent('review-result', { detail: '演示：已收到消息反馈；没有写入数据库。' }))
  return {id:'demo-feedback',messageId,conversationId:'preview-conversation',runId:null,sentiment,category:category??null,comment:comment??null,model:null,createdAt:0,updatedAt:0}
}

const approval: ApprovalRecord = {
  id: 'preview-approval', toolCallId: 'preview-call', runId: 'preview-run', conversationId: 'preview-conversation',
  toolName: 'run_command', status: 'pending', requestedAction: '执行命令', decision: null, requestedAt: 0, resolvedAt: null,
  request: { command: 'git status', cwd: 'D:/fox-test/测试项目', availableDecisions: ['allow_once', 'allow_project_rule', 'deny'], projectRule: { eligible: true, tool: 'run_command', suggestedPrefix: 'git status' } },
}
const initialMessages: ThreadMessageLike[] = [
  { id: 'preview-user', role: 'user', content: [{type:'text',text:'请检查项目状态。'}], createdAt: new Date('2026-11-01T12:00:00Z') },
  { id: 'preview-assistant', role: 'assistant', content: [{type:'text',text:'已检查项目状态。你可以将鼠标移动到这一条回复上，查看下方操作。'}], createdAt: new Date('2026-11-01T12:00:01Z'), status: {type:'complete',reason:'stop'} },
]

export function MessagePreview({ onResult }: { onResult: (text:string)=>void }) {
  const [messages,setMessages] = useState(initialMessages)
  const runtime = useExternalStoreRuntime({ messages, isRunning:false, convertMessage: message=>message,
    onNew: async () => { onResult('演示：提交消息，不调用模型。') },
    onEdit: async message => { setMessages(current=>[{...current[0],content:message.content},current[1]]); onResult('演示：用户消息已编辑。') },
    onReload: async () => { onResult('演示：重新生成入口已触发，不调用模型。') },
  })
  return <AssistantRuntimeProvider runtime={runtime}><ThreadPrimitive.Root className="fox-thread review-message-thread"><ThreadPrimitive.Messages components={{UserMessage:FoxUserMessage,AssistantMessage:FoxAssistantMessage}}/></ThreadPrimitive.Root></AssistantRuntimeProvider>
}

export function ComposerPreview({ kind, after, status, project, onResult }: { kind:'composer'|'attachment'|'approval'; after:boolean; status:AgentBackendStatus; project:ProjectRecord; onResult:(text:string)=>void }) {
  const [draft,setDraft] = useState('')
  const [agent,setAgent] = useState<AgentBackendId>('fox')
  const [reset,setReset] = useState(0)
  const [resolved,setResolved] = useState(false)
  const [attachmentMode,setAttachmentMode] = useState<'knowledge'|'file'>('knowledge')
  const fileRequest = useMemo(()=>kind==='attachment'&&attachmentMode==='file'?{requestId:reset+1,files:[new File(['hello'],'项目记录.txt',{type:'text/plain'})]}:null,[kind,attachmentMode,reset])
  const knowledge = useMemo(()=>({knowledgeBindings:[],knowledgeReferences:kind==='attachment'&&attachmentMode==='knowledge'?[{source:'local' as const,providerKey:'local' as const,id:'94d5990a-c491-46-项目记录'}]:[],onKnowledge:()=>onResult('演示：打开资料库选择器')}),[kind,attachmentMode,onResult])
  const entry = useMemo(()=>({projects:[project],onSelectProject:()=>onResult('演示：已选择测试项目'),onAddProject:()=>onResult('演示：此处打开项目文件夹选择器'),draftAgentBackend:agent,agentBackendStatus:status,onSelectAgentBackend:setAgent,onOpenModelSettings:()=>onResult('模型配置请查看第 9 项研究结论。')}),[agent,status,project,onResult])
  const resolve = async (_id:string,decision:string) => { setResolved(true);onResult(`演示审批：${decision}；没有执行命令。`);return true }
  useEffect(()=>{ const listener=(event:Event)=>{onResult((event as CustomEvent<string>).detail);if(kind==='approval')setResolved(true)};window.addEventListener('review-result',listener);return()=>window.removeEventListener('review-result',listener) },[kind,onResult])
  const showApproval = kind==='approval'&&!resolved
  return <TooltipProvider><ComposerEntryContext.Provider value={entry}><ComposerRuntimeContext.Provider value={knowledge}>
    <div className="review-sample-controls"><Button variant="outline" size="sm" onClick={()=>{setReset(v=>v+1);setDraft('');setResolved(false);onResult('示例已重置')}}>重置示例</Button>{kind==='attachment'&&<><Button variant={attachmentMode==='knowledge'?'secondary':'ghost'} size="sm" onClick={()=>{setAttachmentMode('knowledge');setReset(v=>v+1)}}>知识库引用</Button><Button variant={attachmentMode==='file'?'secondary':'ghost'} size="sm" onClick={()=>{setAttachmentMode('file');setReset(v=>v+1)}}>文件附件</Button></>}</div>
    <div className={`review-real-stage fox-chat-stage ${kind!=='approval'?'is-empty':''}`}>
      {kind!=='approval'&&<div className="review-welcome"><img src="/mascot/fox_sit.png" alt=""/><div><h2>今天想做什么？</h2><p>Fox 可以和你对话、处理本机文件，也可以从资料库中检索资料。</p></div></div>}
      {kind==='approval'&&<div className="review-approval-flow"><p>需要执行命令来检查项目状态。</p>{after&&showApproval&&<ProposedApproval key={`after-${reset}`} approval={approval} onResolve={resolve}/ >}{resolved&&<p role="status">本次审批演示已处理。</p>}</div>}
      <MemoizedComposer key={`${kind}-${reset}`} resetKey={reset} draft={draft} setDraft={setDraft} chatState="complete" runtimeControlled newConversation={kind!=='approval'} centered={false} projectRoot={project.rootPath} projectPermissionMode="ask" initialAttachments={fileRequest} runtimeApprovals={!after&&showApproval?[approval]:[]} onResolveApproval={resolve} onSubmitPrompt={async()=>{if(after&&showApproval){onResult('请先处理上方审批；没有提交消息。');return 'approval' as const}onResult('演示：提交成功，没有调用模型。');return 'complete' as const}}/>
    </div>
  </ComposerRuntimeContext.Provider></ComposerEntryContext.Provider></TooltipProvider>
}
