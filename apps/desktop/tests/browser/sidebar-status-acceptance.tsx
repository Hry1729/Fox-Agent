// Browser-only acceptance scene. The Sidebar, status icon, grouping, pin hook,
// and desktop client contract are production code; Host replies are test data.
import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Sidebar } from '../../src/features/chat/workbench'
import { ConversationStatusIcon } from '../../src/features/chat/conversation-status-icon'
import { useProjectCatalog } from '../../src/features/conversations/hooks/use-project-catalog'
import { desktopClient } from '../../src/features/conversations/api/desktop-client'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import type { ConversationSummary, ProjectRecord } from '../../src/features/conversations/model/types'
import type { ConversationRunIndicator } from '../../src/features/chat/conversation-run-indicator'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'
import '../../src/styles/workspace-pages.css'

let catalog:ProjectRecord[]=[
  {id:'alpha',name:'Alpha',rootPath:'D:/acceptance/Alpha',permissionMode:'ask',status:'active',createdAt:1,updatedAt:1,lastOpenedAt:null,pinned:false},
  {id:'beta',name:'Beta',rootPath:'D:/acceptance/Beta',permissionMode:'ask',status:'active',createdAt:2,updatedAt:2,lastOpenedAt:null,pinned:false},
]
desktopClient.listProjects=async()=>catalog.map(project=>({...project}))
desktopClient.setProjectPinned=async(id,pinned)=>{
  catalog=catalog.map(project=>project.id===id?{...project,pinned}:project)
  const project=catalog.find(project=>project.id===id)
  if(!project)throw new Error('Missing test project')
  return {...project}
}
const summary=(id:string,title:string,projectId:string|null):ConversationSummary=>({
  id,title,projectId,projectRoot:projectId?`D:/acceptance/${projectId==='alpha'?'Alpha':'Beta'}`:null,
  agentId:'fox-general',agentName:'Fox',permissionMode:'ask',status:'active',createdAt:10,updatedAt:10,lastMessageAt:10,
})
const conversations=[summary('beta-running','运行中的任务','beta'),summary('idle','新的讨论','alpha'),summary('waiting','确认操作或选择选项','alpha'),summary('completed','已经完成的任务','alpha'),summary('regular','普通对话：正在执行',null)]
const indicators=new Map<string,ConversationRunIndicator>([['beta-running','running'],['waiting','approval'],['completed','completed'],['regular','running']])
const noop=()=>{}
function Scene(){
  const {projects,setProjectPinned}=useProjectCatalog(true)
  const [selected,setSelected]=useState('idle'),[dark,setDark]=useState(false),[collapsed,setCollapsed]=useState(false),[reversed,setReversed]=useState(false)
  document.documentElement.classList.toggle('dark',dark)
  return <div className="fox-shell acceptance-scene" data-theme={dark?'dark':'light'}>
    <Sidebar collapsed={collapsed} assistantMode="assistant" onModeChange={noop} onNewChat={noop} onNewProjectChat={noop} onAddProject={noop} onOpenConversation={id=>setSelected(id??'idle')} onRenameConversation={noop} onPinConversation={noop} onPinProject={project=>{if(project.id)void setProjectPinned(project.id,!project.pinned)}} onArchiveConversation={noop} onUnarchiveConversation={noop} onTrashConversation={noop} onRestoreConversation={noop} onPurgeConversation={noop} onDeleteProject={noop} onNavigate={noop} onExitManagement={noop} onExpand={()=>setCollapsed(false)} onTheme={()=>setDark(!dark)} onSettings={noop} projects={projects} runtimeConversations={reversed?[...conversations].reverse():conversations} activeConversationId={selected} newChatActive={false} runIndicators={indicators} activeView="chat"/>
    <main className="acceptance-main"><p>Fox 侧栏界面验收 · 模拟对话数据</p><h1>9 点阵与项目置顶</h1><div className="acceptance-controls"><button onClick={()=>setDark(!dark)}>切换深浅色</button><button onClick={()=>setCollapsed(!collapsed)}>收起或展开侧栏</button><button onClick={()=>setReversed(!reversed)}>反转对话活动顺序</button></div><section className="acceptance-icons">{(['idle','running','waiting','complete'] as const).map(state=><div key={state}><ConversationStatusIcon state={state} size={56}/><span>{({idle:'空闲：留空',running:'运行：中心涟漪',waiting:'等待：黄色回声',complete:'结束：绿色勾形呼吸'})[state]}</span></div>)}</section><p>此场景直接使用 Fox 生产组件。置顶在项目的“更多”菜单和右键菜单中操作。数据存储由测试适配器提供；真实数据库持久化由 Rust 回归测试验证。</p></main>
    <style>{`.acceptance-scene{display:flex;height:100vh;min-height:700px}.acceptance-scene>.fox-sidebar{width:288px;flex:0 0 288px;height:100vh}.acceptance-main{padding:48px;flex:1;color:var(--fox-text)}.acceptance-main h1{font-size:28px;font-weight:600;margin:14px 0 24px}.acceptance-main p{font-size:13px;line-height:1.9;max-width:760px;color:var(--fox-muted)}.acceptance-controls{display:flex;gap:10px;margin-bottom:30px}.acceptance-controls button{border:1px solid var(--fox-border);border-radius:7px;padding:9px 13px;background:var(--fox-panel);font-size:12px}.acceptance-icons{display:flex;gap:34px;padding:32px 0}.acceptance-icons>div{display:flex;flex-direction:column;align-items:center;justify-content:center;gap:18px;min-width:130px;min-height:100px}.acceptance-icons>div:first-child{padding-top:56px}.acceptance-icons span{font-size:12px}`}</style>
  </div>
}
createRoot(document.querySelector('#root')!).render(<TooltipProvider><Scene/></TooltipProvider>)
