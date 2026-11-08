import React, { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Brain, ChevronRight, Sun, Moon, FileText, CircleSlash2 } from 'lucide-react'
import { ComposerAgentPicker } from '../src/features/chat/composer/composer-agent-picker'
import { ComposerProjectBar } from '../src/features/chat/composer/composer-project-bar'
import { RuntimeArtifacts } from '../src/features/chat/runtime-artifacts'
import { StoppedRun } from '../src/components/assistant-ui/elements/error-state'
import { SettingsGroup, SettingsRow } from '../src/features/settings/settings-layout'
import { Button } from '../src/components/ui/button'
import { Switch } from '../src/components/ui/switch-implementation'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../src/components/ui/select'
import { TooltipProvider } from '../src/components/ui/tooltip'
import { ComposerPreview, MessagePreview } from './chat-preview'
import { ServiceProposal } from './service-proposal'
import { FileCardProposal } from './file-card-proposal'
import type { AgentBackendId, AgentBackendStatus } from '../src/features/conversations/model/agent-backend'
import type { ProjectRecord } from '../src/features/conversations/model/types'
import './component-adjustments.css'
import './review.css'
import './refinements.css'

const items = [
  { id:'composer', number:'01', title:'输入框底部留白', image:'01', group:'输入与选择', note:'底栏收窄至 32px，项目与 Fox 胶囊等高；悬停底色到外框两侧和底部均留 6px，保留输入区高度。', source:'composer/fox-composer.tsx · areas/composer.css', reuse:'完整 Composer、项目栏、Agent 选择器', action:'输入文字，或切换“当前实现”查看底栏高度变化。' },
  { id:'agent', number:'02', title:'Agent 选择菜单', image:'02', group:'输入与选择', note:'名称和说明同排。次要说明使用弱化文字，选中标记沿用现有菜单行为。', source:'composer/composer-agent-picker.tsx · workbench.css', reuse:'DropdownMenu / DropdownMenuItem', action:'点击 Fox 打开菜单，选择不同 Agent。' },
  { id:'project', number:'03', title:'项目菜单间距', image:'03', group:'输入与选择', note:'搜索、列表和底部动作只保留一层外距。项目与添加行共用图标、文字对齐线，分隔线上下留白相等。', source:'composer/composer-project-bar.tsx · workbench.css', reuse:'Popover、Command、CommandGroup', action:'打开项目菜单，搜索项目或点击添加项目。' },
  { id:'messages', number:'04', title:'取消状态与消息操作', image:'04', group:'对话与交付', note:'对话区域保持居中；取消状态使用圆圈斜线图标，与思维链共用图标列和文字起点。消息操作仍按条悬停显示。', source:'thread/fox-assistant-message.tsx · thread/fox-user-message.tsx · error-state.tsx', reuse:'assistant-ui 消息与操作栏、StoppedRun', action:'将鼠标移到用户消息或助手消息上；也可用 Tab 进入操作。' },
  { id:'file', number:'05', title:'交付文件卡片', image:'05', group:'对话与交付', note:'文件图标独立放入轻底色容器，名称与类型、大小分层；移除重复分组标题和说明，保留轻量打开入口。', source:'chat/runtime-artifacts.tsx', reuse:'RuntimeArtifacts、File、现有文件图标', action:'点击文件卡片查看演示内容。' },
  { id:'attachment', number:'06', title:'附件与引用圆角', image:'06', group:'输入与选择', note:'截图中实际是知识库引用 chip。方案按外框内距计算内层圆角，并将文件附件接入相同的主题 token。名称数据另行核对。', source:'composer/fox-composer.tsx · workbench.css · ai-elements/attachments.tsx', reuse:'原知识库 chip、AI Elements Attachment', action:'对比附件与输入框的弧度；可直接编辑下方输入区。' },
  { id:'approval', number:'07', title:'审批移入对话', image:'07', group:'对话与交付', note:'审批放在对话流中，输入框保持独立。项目常驻授权与其说明收进“查看操作详情”，主区域保留命令和本次决策。', source:'RuntimeApprovalPrompt.tsx · fox-composer.tsx · fox-thread.tsx', reuse:'原审批决策组件、FoxDisclosure', action:'展开操作详情，或点击拒绝 / 只允许这一次。此页不会执行命令。' },
  { id:'settings', number:'08', title:'设置分组容器', image:'08', group:'设置与连接', note:'每组标题放到圆角边框外左上角。组内行用轻分隔线，沿用当前主题、控件和字号。', source:'settings/settings-layout.tsx · settings-layout.css', reuse:'SettingsGroup、SettingsRow、Select、Switch', action:'切换主题、文字大小或开关，检查分组密度和深色模式。' },
  { id:'services', number:'09', title:'模型服务与 Agent 配置', image:null, group:'设置与连接', note:'把“服务信息”和“Agent 使用来源”分开表达。添加与编辑共用页内表单，协议细节收进高级设置；读取、导入、绑定和实际验证分别呈现。', source:'settings-pages.tsx · agent-backend-settings.tsx · external_model.rs', reuse:'现有服务表单与配置导入能力', action:'演示从本机导入、配置未设置的 Agent，以及统一添加 / 编辑服务流程。' },
] as const
const status: AgentBackendStatus = { switchEnv:'', anyExternalEnabled:true, unknownEntries:[], options:(['fox','pi','codex','deepseek'] as const).map(backend=>({backend, enabled:true, wired:true, experimental:backend==='deepseek', reason:null})) }
const project: ProjectRecord = { id:'preview', name:'测试项目', rootPath:'D:\\fox-test\\测试项目', permissionMode:'ask', status:'active', createdAt:0, updatedAt:0, lastOpenedAt:null }

function App() {
  const [id,setId] = useState<string>(new URLSearchParams(location.search).get('item') || 'composer')
  const item = items.find(entry=>entry.id===id) || items[0]
  const [after,setAfter] = useState(true)
  const [original,setOriginal] = useState(false)
  const [dark,setDark] = useState(false)
  const [theme,setTheme] = useState('graphite')
  const [agent,setAgent] = useState<AgentBackendId>('fox')
  const [feedback,setFeedback] = useState('')
  const [enabled,setEnabled] = useState(true)
  const [textScale,setTextScale] = useState('100%')
  const [fileOpen,setFileOpen] = useState(false)
  function select(value:string) { setId(value); setFeedback('');setFileOpen(false);setOriginal(false);history.replaceState(null,'',`?item=${value}`) }
  function version(value:boolean) {setAfter(value);document.documentElement.dataset.review=value?'after':'before';setFeedback('')}
  return <TooltipProvider><div className="design-review">
    <aside className="design-nav">
      <a className="design-brand" href="?item=composer"><img src="/avatars/fox-assistant.png" alt=""/><span>Fox<span className="design-brand-sub">界面优化评审</span></span></a>
      <nav aria-label="评审项目">{['输入与选择','对话与交付','设置与连接'].map(group=><div className="design-nav-group" key={group}><p>{group}</p>{items.filter(entry=>entry.group===group).map(entry=><button key={entry.id} className={item.id===entry.id?'active':''} aria-current={item.id===entry.id?'page':undefined} onClick={()=>select(entry.id)}><span className="design-number">{entry.number}</span><span>{entry.title}</span></button>)}</div>)}</nav>
      <div className="design-nav-footer"><span>源码基线 bb6f88a9</span><span>本地 HEAD 5068da21</span><span>仅预览 · 示例数据</span></div>
    </aside>
    <main className="design-main">
      <header className="design-topbar"><span>Fox / UI 评审 / <strong>{item.number}</strong></span><div><Select value={theme} onValueChange={value=>{setTheme(value);document.documentElement.dataset.colorTheme=value}}><SelectTrigger aria-label="预览主题"><SelectValue/></SelectTrigger><SelectContent><SelectItem value="graphite">石墨</SelectItem><SelectItem value="mist">雾蓝</SelectItem><SelectItem value="paper">暖纸</SelectItem></SelectContent></Select><Button variant="ghost" size="icon" aria-label={dark?'切换浅色':'切换深色'} onClick={()=>{setDark(!dark);document.documentElement.classList.toggle('dark',!dark)}}>{dark?<Sun/>:<Moon/>}</Button></div></header>
      <section className="design-heading"><div><p className="design-eyebrow">{item.group} <span> / {item.number}</span></p><h1>{item.title}</h1><p>{item.note}</p></div><span className={`design-scope ${item.id==='services'?'backend':''}`}>{item.id==='services'?'涉及后端 · 流程提案':'前端可独立完成'}</span></section>
      <div className="design-stage-toolbar"><div className="design-segments" role="group" aria-label="对比版本"><button aria-pressed={after} onClick={()=>version(true)}>优化方案</button><button aria-pressed={!after} onClick={()=>version(false)}>当前实现</button></div>{item.image&&<Button variant="ghost" size="sm" onClick={()=>setOriginal(!original)}>{original?'收起原始截图':'查看原始截图'}<ChevronRight className={original?'rotate':''}/></Button>}</div>
      {original&&item.image&&<figure className="design-original"><figcaption>你提供的原始截图 · 用于对照</figcaption><img src={`./evidence/${item.image}.png`} alt={`${item.title}的原始截图`}/></figure>}
      <div className="design-stage" data-surface={item.id} key={`${item.id}-${after}`}>
        <div className="design-stage-label"><span>{after?'优化方案':'当前实现'}</span><span>{item.id==='services'?'流程提案 · 模拟状态':'真实组件预览 · 不连接桌面后端'}</span></div>
        {['composer','attachment','approval'].includes(item.id)&&<ComposerPreview kind={item.id as 'composer'|'attachment'|'approval'} after={after} status={status} project={project} onResult={setFeedback}/>}
        {item.id==='agent'&&<div className="design-picker-scene"><div className="design-picker-caption"><p>本次新对话</p><span>由哪个 Agent 处理？</span></div><div className="design-picker-anchor"><ComposerAgentPicker value={agent} status={status} onSelect={setAgent}/></div></div>}
        {item.id==='project'&&<div className="design-project-scene"><ComposerProjectBar projectRoot={project.rootPath} projects={[project]} onSelect={()=>setFeedback('已选择测试项目')} onAdd={()=>setFeedback('正式应用会打开文件夹选择器；本预览不访问本机文件。')}/></div>}
        {item.id==='messages'&&<div className="design-message-scene"><MessagePreview onResult={setFeedback}/><div className="design-stopped"><p className="design-process">{after&&<Brain aria-hidden="true"/>}<span>深度思考 · 正在检查项目状态…</span></p>{after?<div data-slot="stopped-run" className="aui-stopped-run max-w-none" role="status"><div><span><CircleSlash2 aria-hidden="true"/>运行已取消，已生成内容保留</span></div></div>:<StoppedRun reason="运行已取消，已生成内容保留"/>}</div></div>}
        {item.id==='file'&&<div className="design-file-scene"><p>已在项目根目录创建 <code>p0.txt</code>，内容为 <code>hello</code>。</p>{after?<FileCardProposal onOpen={()=>setFileOpen(!fileOpen)}/>: <RuntimeArtifacts artifacts={[{id:'preview-file',conversationId:'preview',runId:null,displayName:'p0.txt',artifactType:'file',artifactClass:'deliverable',artifactOrigin:'project',storagePath:'D:\\fox-test\\测试项目\\p0.txt',mediaType:'text/plain',byteSize:5,sha256:null,status:'ready',createdAt:0,updatedAt:0,declaredAt:1,declaredDescription:'内容为 hello 的文本文件'}]} onOpenArtifact={()=>setFileOpen(!fileOpen)}/>}{fileOpen&&<div className="design-file-content"><span><FileText size={16}/> p0.txt</span><pre>hello</pre></div>}</div>}
        {item.id==='settings'&&<div className="fox-settings-page review-settings design-settings">
          <SettingsGroup title="外观"><div className={after?'review-settings-body':''}><SettingsRow title="配色主题" description="选择 Fox 界面的配色"><Select value={theme} onValueChange={value=>{setTheme(value);document.documentElement.dataset.colorTheme=value}}><SelectTrigger aria-label="配色主题"><SelectValue/></SelectTrigger><SelectContent><SelectItem value="graphite">石墨</SelectItem><SelectItem value="mist">雾蓝</SelectItem><SelectItem value="paper">暖纸</SelectItem></SelectContent></Select></SettingsRow><SettingsRow title="文字大小"><Select value={textScale} onValueChange={setTextScale}><SelectTrigger aria-label="文字大小"><SelectValue/></SelectTrigger><SelectContent>{['90%','100%','110%'].map(value=><SelectItem key={value} value={value}>{value}</SelectItem>)}</SelectContent></Select></SettingsRow></div></SettingsGroup>
          <SettingsGroup title="对话"><div className={after?'review-settings-body':''}><SettingsRow title="显示思考过程" description="在回复中展开执行过程"><Switch aria-label="显示思考过程" checked={enabled} onCheckedChange={setEnabled}/></SettingsRow><SettingsRow title="发送方式"><Select defaultValue="enter"><SelectTrigger aria-label="发送方式"><SelectValue/></SelectTrigger><SelectContent><SelectItem value="enter">Enter 发送</SelectItem><SelectItem value="mod">Ctrl + Enter 发送</SelectItem></SelectContent></Select></SettingsRow></div></SettingsGroup>
          <SettingsGroup title="数据与关于"><div className={after?'review-settings-body':''}><SettingsRow title="使用记录" description="查看模型用量和对话记录"><Button variant="outline" size="sm" onClick={()=>setFeedback('使用记录沿用现有页面；本次只统一外层分组样式。')}>查看记录<ChevronRight size={14}/></Button></SettingsRow></div></SettingsGroup>
        </div>}
        {item.id==='services'&&(after?<ServiceProposal/>:<div className="design-current-flow"><h2>当前源码中的配置路径</h2><p>模型服务列表 → 添加服务弹窗 / 编辑服务抽屉 → 添加模型弹窗</p><p>高级设置 → 选择协议 → 设置默认 Agent → 再判断连接是否可用</p><p>本机配置导入与模型服务分属不同入口，部分 Agent 的导入能力尚未接入界面。</p><Button onClick={()=>version(true)}>查看统一流程方案</Button></div>)}
      </div>
      <div className="design-help"><p>{item.action}</p><output aria-live="polite">{feedback}</output></div>
      <details className="design-source"><summary>实现依据与复用范围</summary><div><p><strong>源码：</strong><code>{item.source}</code></p><p><strong>复用：</strong>{item.reuse}</p><p>{item.id==='services'?'此项需要先补后端状态与绑定能力，交互使用模拟数据。':'本方案只调整本页评审作用域，尚未应用到 Fox 产品代码。'}</p></div></details>
      <footer className="design-footer">9 项问题按你的描述编号；项目菜单对应原截图 4，取消状态对应原截图 3。预览使用当前代码、主题 token 与图标库。</footer>
    </main>
  </div></TooltipProvider>
}

export function mount(){ document.documentElement.dataset.review='after';createRoot(document.getElementById('root')!).render(<App/>) }

