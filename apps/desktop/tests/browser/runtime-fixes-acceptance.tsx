// Real Sidebar, approval prompt, icon and notification hooks; simulated Host data.
import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Sidebar } from '../../src/features/chat/workbench'
import { RuntimeApprovalPrompt } from '../../src/features/chat/components/RuntimeApprovalPrompt'
import { ConversationStatusIcon } from '../../src/features/chat/conversation-status-icon'
import { useConversationCompletionNotices, useVisibleCompletionAcknowledgement } from '../../src/features/chat/use-conversation-completion-notices'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import type { ApprovalRecord, ConversationSummary } from '../../src/features/conversations/model/types'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'

const noop=()=>{}
const storageValues=new Map<string,string>()
const storage={getItem:(key:string)=>storageValues.get(key)??null,setItem:(key:string,value:string)=>{storageValues.set(key,value)}}
const initial:ConversationSummary[]=[
  ['running-a','当前任务','running'],['running-b','另一个运行任务','running'],['waiting-a','等待审批的任务','approval'],['waiting-b','另一项等待审批','approval'],['completed','刚刚完成的任务','completed'],
].map(([id,title,status])=>({id,title,agentId:'fox-general',agentName:'Fox',projectId:null,projectRoot:null,status:'active',createdAt:1,updatedAt:1,lastMessageAt:1,lastRunId:`${id}-1`,lastRunStatus:status==='completed'?'completed':'running',activeRunId:status==='completed'?null:`${id}-1`,activeRunStatus:'running',awaitingApproval:status==='approval'}))
const longCommand='python -c "'+Array.from({length:48},(_,i)=>`print('第 ${i+1} 组：读取文件、核对内容、计算并显示结果，不修改任何真实文件');`).join(' ')+'"'
const approval={id:'visual-approval:v1',toolCallId:'visual-tool',runId:'visual-run',conversationId:'visual',toolName:'run_command',status:'pending',requestedAction:'run_command',request:{tool:'run_command',input:{command:longCommand},availableDecisions:['allow_once','deny']},requestedAt:1,resolvedAt:null} as ApprovalRecord
function Scene(){
  const [conversations,setConversations]=useState(initial),[selected,setSelected]=useState('running-a'),[visible,setVisible]=useState(true),[dark,setDark]=useState(false),[late,setLate]=useState(false),[feedback,setFeedback]=useState(''),[clocks,setClocks]=useState('')
  const {runIndicators,acknowledgeCompletion}=useConversationCompletionNotices(conversations,new Map(),0,storage)
  const acknowledge=useVisibleCompletionAcknowledgement({visible,conversationId:selected,displayedConversationId:selected,acknowledgeCompletion})
  document.documentElement.classList.toggle('dark',dark)
  const finish=()=>setConversations(items=>items.map(item=>item.id===selected?{...item,activeRunId:null,lastRunStatus:'completed',awaitingApproval:false}:item))
  return <main className="fox-shell runtime-acceptance" data-theme={dark?'dark':'light'} onPointerDownCapture={acknowledge} onWheelCapture={acknowledge} onKeyDownCapture={acknowledge}>
    <Sidebar collapsed={false} assistantMode="assistant" onModeChange={noop} onNewChat={noop} onNewProjectChat={noop} onAddProject={noop} onOpenConversation={id=>{if(id)setSelected(id)}} onRenameConversation={noop} onPinConversation={noop} onArchiveConversation={noop} onUnarchiveConversation={noop} onTrashConversation={noop} onRestoreConversation={noop} onPurgeConversation={noop} onDeleteProject={noop} onNavigate={()=>setVisible(false)} onExitManagement={()=>setVisible(true)} onExpand={noop} onTheme={()=>setDark(value=>!value)} onSettings={()=>setVisible(false)} runtimeConversations={conversations} activeConversationId={selected} newChatActive={false} runIndicators={runIndicators} activeView="chat"/>
    <section className="acceptance-content"><p>Fox 生产组件 · 模拟验证数据</p><h1>状态与审批验证</h1><nav><button onClick={finish}>完成当前任务</button><button onClick={()=>setVisible(!visible)}>{visible?'离开对话页面':'返回对话页面'}</button><button onClick={()=>setLate(true)}>添加另一个运行图标</button><button onClick={()=>setDark(!dark)}>切换深浅色</button></nav>
    <p>{visible?'当前对话页面':'其他页面'} · 完成通知保留到点击、滚动或返回页面。绿色完成状态使用慢速 Glyph Pulse。</p><div className="acceptance-motion"><ConversationStatusIcon state="running"/><ConversationStatusIcon state="running"/><ConversationStatusIcon state="waiting"/><ConversationStatusIcon state="waiting"/><ConversationStatusIcon state="complete"/><ConversationStatusIcon state="complete"/>{late&&<><ConversationStatusIcon state="running"/><ConversationStatusIcon state="complete"/></>}</div>
    <button onClick={()=>setClocks(JSON.stringify(Array.from(document.querySelectorAll('.acceptance-motion .fox-status-matrix')).map(icon=>({state:icon.getAttribute('data-icon-state'),dotOpacities:Array.from(icon.querySelectorAll<HTMLElement>('.dmx-dot')).map(dot=>dot.style.opacity),animations:icon.getAnimations({subtree:true}).filter(animation=>'animationName' in animation).map(animation=>({startTime:animation.startTime,currentTime:animation.currentTime}))}))))}>检查动画同步</button><output data-testid="clock-report" hidden={!clocks}>{clocks}</output>
    {!feedback&&<div className="fox-prompt-shell is-decision is-approval"><div className="fox-decision-card"><div className="fox-decision-approval-list"><RuntimeApprovalPrompt approval={approval} onResolve={async(_id,decision)=>{setFeedback(decision==='deny'?'已拒绝模拟请求':'已允许模拟请求');return true}}/></div></div></div>}<p role="status">{feedback}</p></section>
    <style>{`.runtime-acceptance{display:flex;min-height:100vh}.runtime-acceptance>.fox-sidebar{width:288px;flex:0 0 288px;height:100vh}.acceptance-content{padding:32px;flex:1;min-width:0;color:var(--fox-text)}.acceptance-content h1{font-size:24px;margin:10px 0 20px}.acceptance-content>p{font-size:13px;margin:14px 0;color:var(--fox-muted)}.acceptance-content nav{display:flex;gap:10px;flex-wrap:wrap}.acceptance-content nav button{padding:8px 12px;border:1px solid var(--fox-border);border-radius:7px;background:var(--fox-panel);font-size:12px}.acceptance-motion{display:flex;gap:28px;margin:24px 0}`}</style>
  </main>
}
const root=createRoot(document.querySelector('#root')!)
root.render(<TooltipProvider><Scene/></TooltipProvider>)
if(import.meta.hot) import.meta.hot.dispose(()=>root.unmount())
