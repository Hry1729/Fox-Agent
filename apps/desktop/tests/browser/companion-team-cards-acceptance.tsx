import { mockIPC } from '@tauri-apps/api/mocks'
import { AssistantRuntimeProvider, useExternalStoreRuntime } from '@assistant-ui/react'
import { ContactReplyBubble } from '../../src/features/companions/marks'
import { createRoot } from 'react-dom/client'
import { ContactsContext, type ContactsContextValue } from '../../src/features/companions/context'
import { CompanionTeamActionsProvider, CompanionTeamCards } from '../../src/features/companions/companion-team-cards'
import type { Companion, CompanionTeamAction, CompanionTeamDecision } from '../../src/features/companions/api'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'
import '../../src/features/companions/contacts.css'

const params = new URLSearchParams(location.search)
const dark = params.get('mode') === 'dark'
document.documentElement.dataset.colorTheme = 'graphite'
document.documentElement.classList.toggle('dark', dark)
document.documentElement.style.colorScheme = dark ? 'dark' : 'light'

const companion: Companion = {
  id: 'local-fixture', conversationId: 'local-chat', templateAgentId: null, displayName: '小狐', avatarRef: '🦊',
  duty: '', personaOverride: null, workspaceDir: 'D:/acceptance/fox', workspaceState: 'ready', workspaceError: null,
  isDefault: true, pinnedAt: 1, workShortcutOrder: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'follow_global', lastReadDeliveryOrder: null, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}
const seed = (id: string, patch: Partial<CompanionTeamAction>): CompanionTeamAction => ({
  id, companionId: companion.id, conversationId: companion.conversationId, runId: 'fixture-run', targetId: id,
  targetName: '周报伙伴', kind: 'create', status: 'applied', createdAt: 1, updatedAt: 1, ...patch,
})
const workspacePath = 'D:/acceptance/companion-files/monthly-delivery/weekly-reports'
let actions = [
  seed('create', { after: { duty: '汇总周报和下周计划' } }),
  seed('profile', { kind: 'profile', before: { instructions: '整理工作记录' }, after: { instructions: '按项目汇总工作记录，标出延期风险。' } }),
  seed('model', { kind: 'model', before: { modelRef: '旧模型' }, after: { modelRef: '新模型' } }),
  seed('delete', { kind: 'delete', status: 'pending', targetName: '资料伙伴', preview: {
    planHash: '0ff9bc1b110f02ba512a1587091019ae04a3db54aaf3a7d4e5ec8411ce0e2afd',
    plan: { companionId: 'delete', displayName: '资料伙伴', workspacePath, workspaceFiles: 4, workspaceBytes: 128,
      messages: 12, notes: 5, topics: 3, sharedMemories: 2, generations: 3, grantedProjects: ['每周项目'],
      files: [{ kind: 'workspace', disposition: 'retain', path: `${workspacePath}/november_summary.xlsx`, bytes: 128 }] },
  } }),
  seed('archive', { kind: 'archive', status: 'pending', targetName: '会议伙伴' }),
]
mockIPC((command, args) => {
  if (command === 'companion_team_actions_list') return { ok: true, data: actions }
  if (command === 'companion_team_action_resolve') {
    const { actionId, decision } = (args as { request: { actionId: string; decision: CompanionTeamDecision } }).request
    const status = { undo: 'undone', confirm: 'confirmed', dismiss: 'dismissed' } as const
    actions = actions.map(action => action.id === actionId ? { ...action, status: status[decision] } : action)
    return { ok: true, data: actions.find(action => action.id === actionId) }
  }
  throw new Error(`Unexpected fixture command: ${command}`)
}, { shouldMockEvents: true })

const noop = () => {}
const context: ContactsContextValue = {
  current: companion, privateChat: true, list: { companions: [companion], pendingDeletions: [] },
  sendPrompt: async () => {}, projects: [], model: '本地验收模型', permission: 'ask', error: '',
  refresh: async () => {}, refreshConversation: async () => {}, open: noop, showProfile: noop, leave: noop,
  setPermission: noop, openSharedMemory: noop, askToOrganize: noop,
}

function GuidePreview() {
  const runtime = useExternalStoreRuntime({ messages: [], isRunning: false, onNew: async () => {} })
  return <AssistantRuntimeProvider runtime={runtime}><ContactReplyBubble contact={companion} reply={{ id: 'welcome', text: '你好，我是小狐。可以直接说说你想做什么。', createdAt: 1, guide: { title: '可以从这里开始', options: ['看看你能做什么', '帮我新建一个伙伴', '整理文件和资料', '写材料和汇报'].map(label => ({ label, message: label })) } }} /></AssistantRuntimeProvider>
}

createRoot(document.getElementById('root')!).render(
  <ContactsContext.Provider value={context}>
    <main className="fox-thread is-contact-chat" style={{ minHeight: '100dvh', maxWidth: 800, margin: '0 auto', padding: 20 }}>
      <div className="fox-contact-run-body">
        {params.has('guide') ? <GuidePreview /> : <CompanionTeamActionsProvider><CompanionTeamCards runId="fixture-run" /></CompanionTeamActionsProvider>}
      </div>
    </main>
  </ContactsContext.Provider>,
)
