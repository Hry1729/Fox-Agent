import { createRoot } from 'react-dom/client'
import { Sidebar } from '../../src/features/chat/shell/fox-sidebar'
import { ContactsContext } from '../../src/features/companions/context'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'
import '../../src/styles/workspace-pages.css'
import '../../src/styles/areas/sidebar.css'
const noop = () => {}
const contacts = Array.from({length:20}, (_, i) => ({id:`c${i}`,conversationId:`v${i}`,displayName:`小 Fox ${i+1}`,avatarRef:'🦊',latestMessagePreview:'这是一条预览消息',archivedAt:null,deletingAt:null,pinnedAt:null,pendingActionCount:0,unreadCount:0}))
const context = {list:{companions:contacts,pendingDeletions:[]},refresh:async()=>{},error:'',projects:[],privateChat:false,permission:'ask',model:'',open:noop,showProfile:noop,leave:noop,openSharedMemory:noop,askToOrganize:noop,setPermission:noop,refreshConversation:async()=>{}}
const project = {id:'p1',name:'验收项目',rootPath:'C:/fixture',createdAt:1,pinned:false,permissionMode:'ask',status:'active',updatedAt:1,lastOpenedAt:null}
document.body.style.margin='0'
createRoot(document.getElementById('root')!).render(<TooltipProvider><ContactsContext.Provider value={context as any}><div className="fox-shell" style={{height:'600px',display:'flex'}}><Sidebar collapsed={false} assistantMode="assistant" onModeChange={noop} onNewChat={noop} onNewProjectChat={noop} onAddProject={noop} onOpenConversation={noop} onRenameConversation={noop} onPinConversation={noop} onArchiveConversation={noop} onUnarchiveConversation={noop} onTrashConversation={noop} onRestoreConversation={noop} onPurgeConversation={noop} onDeleteProject={noop} onNavigate={noop} onExitManagement={noop} onExpand={noop} onTheme={noop} onSettings={noop} newChatActive={false} activeView="chat" runtimeConversations={[]} projects={[project as any]} /></div></ContactsContext.Provider></TooltipProvider>)
