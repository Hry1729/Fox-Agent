import { ConversationEmptyPrompt } from '../../src/features/chat/timeline/fox-timeline'
import '../../src/features/chat/thread/fox-thread.css'
import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import { MemoizedComposer } from '../../src/features/chat/composer/fox-composer'
import { ComposerEntryContext } from '../../src/features/chat/composer/composer-entry-context'
import { ComposerRuntimeContext } from '../../src/features/chat/fox-shared'
import { useModelService } from '../../src/features/settings/use-model-service'
import type { AgentRecord, ProjectRecord } from '../../src/features/conversations/model/types'
import '../../src/styles/globals.css'
import '../../src/styles/themes.css'
import '../../src/styles/workbench.css'

const agent = { id: 'fox-general', name: 'Fox 通用助手', runtimeType: 'pi', defaultModel: 'configured-model', resources: { tools: [], skills: [], knowledges: [], mcps: [] }, available: true, isDefault: true, packageManifest: {} } as unknown as AgentRecord
const projects = ['项目甲', '项目乙'].map((name, index) => ({ id: `project-${index}`, name, rootPath: `D:/测试/${name}`, status: 'active', permissionMode: 'ask' })) as ProjectRecord[]
function Fixture() {
  const [draft, setDraft] = useState('')
  const [project, setProject] = useState<string | null>(null)
  const [history, setHistory] = useState(false)
  const [running, setRunning] = useState(false)
  const [expert, setExpert] = useState<AgentRecord | null>(null)
  const [viewed, setViewed] = useState(false)
  const [knowledge, setKnowledge] = useState(false)
  const service = useModelService()
  ;(window as any).composerFixture = { draft, setDraft, project, history, viewed, knowledge, expert, setHistory, setRunning, setProject }
  return <TooltipProvider><div className="fox-app fox-shell" style={{ height: '100vh', background: 'var(--fox-canvas)' }}>
    <main className={`fox-chat-stage ${history ? '' : 'is-empty'}`} style={{ position: 'relative', height: '100vh' }}>
      {!history && <ConversationEmptyPrompt onStart={() => {}} />}
      <ComposerRuntimeContext.Provider value={{ knowledgeBindings: [], onKnowledge: () => setKnowledge(true) }}>
      <ComposerEntryContext.Provider value={{ projects, defaultExpert: agent, onSelectProject: item => setProject(item.rootPath), onAddProject: () => setProject('D:/测试/新增项目'), onViewDefaultExpert: () => setViewed(true) }}>
        <MemoizedComposer resetKey={0} draft={draft} setDraft={setDraft} chatState={running ? 'running' : 'complete'} centered={!history} newConversation={!history} runtimeControlled projectRoot={project} projectPermissionMode="ask" activeAgent={agent} activeExpert={expert} modelService={service.service} runtimeCapabilities={{ imageInput: false }} onViewExpert={() => setViewed(true)} onChangeExpert={() => setExpert({ ...agent, id: 'fox-reviewer', name: '代码审查专家' })} />
      </ComposerEntryContext.Provider>
      </ComposerRuntimeContext.Provider>
    </main>
  </div></TooltipProvider>
}
document.documentElement.dataset.colorTheme = 'graphite'
createRoot(document.getElementById('root')!).render(<Fixture />)
