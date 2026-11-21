import { mockIPC } from '@tauri-apps/api/mocks'
import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import AssistantUiChat from '../../src/features/chat/thread/fox-assistant-ui-chat'
import { CoordinationTopbar, CoordinationClose } from '../../src/features/companions/assignment-thread'
import { CoordinationTranscript } from '../../src/features/companions/coordination-transcript'
import { ContactQuestionCard } from '../../src/features/companions/chat-question'
import { ContactsContext, type ContactsContextValue } from '../../src/features/companions/context'
import type { Companion } from '../../src/features/companions/api'
import type { CompanionAssignment, ConversationDetail, ConversationMessage } from '../../src/features/conversations/model/types'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'
import '../../src/features/chat/thread/fox-thread.css'

const params = new URLSearchParams(location.search)
const dark = params.get('mode') === 'dark'
document.documentElement.dataset.colorTheme = params.get('theme') ?? 'graphite'
document.documentElement.classList.toggle('dark', dark)
document.documentElement.style.colorScheme = dark ? 'dark' : 'light'
const now = Date.now() - 60_000
const companion: Companion = {
  id:'peer', displayName:'人话', conversationId:'peer-private', templateAgentId:null, avatarRef:'人', duty:'协助核对信息',
  personaOverride:null, workspaceDir:'D:/acceptance/peer', workspaceState:'ready', workspaceError:null, isDefault:false,
  pinnedAt:null, workShortcutOrder:null, lastReadOrdinal:0, modelProviderId:null, modelRef:null, reminderMode:'follow_global',
  lastReadDeliveryOrder:null, manualUnreadFromOrder:null, manualUnreadAt:null, readDeliveryOrder:0, pendingActionCount:0,
  unreadCount:0, activeRunStatus:null, archivedAt:null, deletingAt:null,
}
const chief = {...companion,id:'chief',displayName:'小狐',avatarRef:'狐',conversationId:'work',isDefault:true}
const thread = {sourceConversationId:'work',sourceCompanionId:'chief',sourceName:'小狐',recipientCompanionId:'peer'}
const records: CompanionAssignment[] = [0,1,2,3,4].map(index => ({
  ...thread,id:`count-${index}`, rootRunId:'root',sourceRunId:`source-${index}`,recipientConversationId:'coordination',
  recipientName:'人话',title:'轮流报数', instructions:index===0 ? '我报 1，请你报 2。' : index===1 ? '用户说不用逐条汇报。现在我报 3，请你回 4，接着轮流报到 10。' : String(index*2+1),
  result:String(index*2+2),status:'completed',deliveryStatus:'delivered',reply:'resume',
  createdAt:now+(index===0?2:7+(index-1)*2)*1000,updatedAt:now+(index===0?3:8+(index-1)*2)*1000,
}))
const message = (id:string,role:'user'|'assistant',text:string,second:number):ConversationMessage => ({
  id,runId:`run-${id}`,conversationId:'work',role,kind:'text',content:text,status:'completed',ordinal:second,
  createdAt:now+second*1000,updatedAt:now+second*1000,
})
const messages = [message('user1','user','和人话轮流报数，从 1 到 10。',0),message('reply1','assistant','1',1),message('reply3','assistant','3',4),
  message('user2','user','接下来不用逐条发给我，你们互相报到 10 就可以。',5),message('ack','assistant','好，接下来只互相报数，完成后告诉你。',6),
  message('done','assistant','我们已轮流报完 1 到 10。',16)]
const baseDetail:ConversationDetail = {
  conversation:{id:'work',agentId:'fox-general',agentName:'Fox',title:'小狐',projectId:null,projectRoot:null,permissionMode:'ask',status:'active',createdAt:now,updatedAt:now,lastMessageAt:now},
  companionId:'chief',messages,runtimeEvents:[],toolCalls:[],approvals:[],attachments:[],artifacts:[],knowledgeBindings:[],expertBindings:[],
  hasEarlierMessages:false,goals:[],tasks:[],evidence:[],planRevisions:[],reviewFindings:[],acceptances:[],childRuns:[],expertWorkflow:null,expertTeam:null,lastRun:null,runs:[],
}
mockIPC(command => {
  if(command==='companion_assignments_list') return {ok:true,data:records}
  if(command==='companion_team_actions_list') return {ok:true,data:[]}
  throw new Error(`Unexpected command: ${command}`)
},{shouldMockEvents:true})
const noop=()=>{}
function Scene(){
  const [id,setId]=useState(params.get('view')==='coordination'?'coordination':'work')
  const [answer,setAnswer]=useState('')
  const context:ContactsContextValue={current:id==='work'?chief:undefined,privateChat:id==='work',list:{companions:[chief,companion],pendingDeletions:[]},
    projects:[],model:'acceptance',permission:'ask',error:'',refresh:async()=>{},refreshConversation:async()=>{},
    open:noop,showProfile:noop,leave:noop,setPermission:noop,openSharedMemory:noop,askToOrganize:noop,openCoordination:setId,closeCoordination:()=>setId('work')}
  const detail={...baseDetail,conversation:{...baseDetail.conversation,id},...(id==='coordination'?{coordination:thread,companionId:null,messages:[]}: {})}
  return <ContactsContext.Provider value={context}><TooltipProvider>
    <main className="fox-chat-pane" style={{position:'relative',height:'100dvh',maxWidth:1000,margin:'0 auto'}}>
      {id==='coordination'?<><CoordinationTopbar thread={thread}/><CoordinationTranscript detail={detail}/><CoordinationClose thread={thread}/></>
        : <><div style={{padding:'14px',textAlign:'center'}}>小狐 · 本地验收数据</div><div className="fox-chat-stage" style={{'--fox-chat-column-width':'min(780px, calc(100% - 48px))'} as React.CSSProperties}><div className="fox-thread-shell">
          <AssistantUiChat conversation={{enabled:true,detail,streamingText:''} as any} callbacks={{onSubmit:noop}}/>
        </div></div></>}
      {params.get('state')==='approval' && id==='work' && <div style={{padding:'12px 24px'}}><ContactQuestionCard request={{runId:'question',requestId:'question',createdAt:now,status:answer?'answered':'pending',answerText:answer,
        questions:[{questionId:'q',question:'要开启每天晚上 8 点的精选推送吗？',options:[{label:'开启推送',value:'yes'},{label:'暂不开启',value:'no'}],multiSelect:false,allowOther:true}]}}
        onSubmit={async text=>{setAnswer(text);return true}}/></div>}
    </main>
  </TooltipProvider></ContactsContext.Provider>
}
createRoot(document.getElementById('root')!).render(<Scene/>)
