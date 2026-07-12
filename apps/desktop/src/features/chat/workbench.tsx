import { useEffect, useMemo, useRef, useState, type ChangeEvent, type KeyboardEvent, type ReactNode } from 'react'
import {
  Archive,
  AlertTriangle,
  ArrowUp,
  BookOpen,
  Bot,
  Brain,
  Check,
  ChevronDown,
  ChevronRight,
  CircleStop,
  ClipboardList,
  Code2,
  Copy,
  ExternalLink,
  File,
  FileEdit,
  FilePlus2,
  FileText,
  Folder,
  FolderOpen,
  Folders,
  Globe2,
  CircleHelp,
  ImagePlus,
  Library,
  ListTodo,
  Maximize2,
  MessageCircleMore,
  MessageSquarePlus,
  Mic,
  Minus,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  Pause,
  Paperclip,
  Pin,
  Play,
  Plus,
  RotateCcw,
  Search,
  Settings,
  ShieldCheck,
  Sparkles,
  Square,
  Target,
  SunMoon,
  Terminal,
  ThumbsDown,
  ThumbsUp,
  Trash2,
  UserRound,
  X
} from 'lucide-react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { Switch } from '@/components/ui/switch'
import { Sheet, SheetContent } from '@/components/ui/sheet'
import { Command, CommandEmpty, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import {
  Conversation,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton
} from '@/components/ai-elements/conversation'
import {
  Message,
  MessageAction,
  MessageActions,
  MessageContent,
  MessageResponse
} from '@/components/ai-elements/message'
import { Source, Sources, SourcesContent, SourcesTrigger } from '@/components/ai-elements/sources'
import {
  InlineCitation,
  InlineCitationCard,
  InlineCitationCardBody,
  InlineCitationCardTrigger,
  InlineCitationCarousel,
  InlineCitationCarouselContent,
  InlineCitationCarouselHeader,
  InlineCitationCarouselIndex,
  InlineCitationCarouselItem,
  InlineCitationCarouselNext,
  InlineCitationCarouselPrev,
  InlineCitationQuote,
  InlineCitationSource,
  InlineCitationText
} from '@/components/ai-elements/inline-citation'
import {
  Artifact,
  ArtifactAction,
  ArtifactActions,
  ArtifactContent,
  ArtifactDescription,
  ArtifactHeader,
  ArtifactTitle
} from '@/components/ai-elements/artifact'
import {
  CodeBlock,
  CodeBlockActions,
  CodeBlockCopyButton,
  CodeBlockFilename,
  CodeBlockHeader,
  CodeBlockTitle
} from '@/components/ai-elements/code-block'
import {
  ChainOfThought,
  ChainOfThoughtContent,
  ChainOfThoughtHeader,
  ChainOfThoughtSearchResult,
  ChainOfThoughtSearchResults,
  ChainOfThoughtStep
} from '@/components/ai-elements/chain-of-thought'
import {
  Confirmation,
  ConfirmationAction,
  ConfirmationActions,
  ConfirmationRejected,
  ConfirmationRequest,
  ConfirmationTitle
} from '@/components/ai-elements/confirmation'
import {
  Attachment,
  AttachmentInfo,
  AttachmentPreview,
  AttachmentRemove,
  Attachments
} from '@/components/ai-elements/attachments'
import { FileTree, FileTreeFile, FileTreeFolder } from '@/components/ai-elements/file-tree'
import { Suggestion, Suggestions } from '@/components/ai-elements/suggestion'
import {
  Context as AIContext,
  ContextContent as AIContextContent,
  ContextContentBody as AIContextContentBody,
  ContextContentFooter as AIContextContentFooter,
  ContextContentHeader as AIContextContentHeader,
  ContextTrigger as AIContextTrigger
} from '@/components/ai-elements/context'
import {
  Queue,
  QueueItem,
  QueueItemActions,
  QueueItemContent,
  QueueItemDescription,
  QueueItemIndicator,
  QueueList
} from '@/components/ai-elements/queue'
import {
  Terminal as AITerminal,
  TerminalActions as AITerminalActions,
  TerminalContent as AITerminalContent,
  TerminalCopyButton as AITerminalCopyButton,
  TerminalHeader as AITerminalHeader,
  TerminalStatus as AITerminalStatus,
  TerminalTitle as AITerminalTitle
} from '@/components/ai-elements/terminal'
import {
  WebPreview,
  WebPreviewNavigation,
  WebPreviewNavigationButton,
  WebPreviewUrl
} from '@/components/ai-elements/web-preview'
import { Agent as AIAgent, AgentContent as AIAgentContent, AgentHeader as AIAgentHeader } from '@/components/ai-elements/agent'
import {
  PromptInput,
  PromptInputActionAddAttachments,
  PromptInputActionMenu,
  PromptInputActionMenuContent,
  PromptInputActionMenuItem,
  PromptInputActionMenuTrigger,
  PromptInputBody,
  PromptInputButton,
  PromptInputFooter,
  PromptInputSelect,
  PromptInputSelectContent,
  PromptInputSelectItem,
  PromptInputSelectTrigger,
  PromptInputSelectValue,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
  usePromptInputAttachments
} from '@/components/ai-elements/prompt-input'
import { citations, conversations, files } from './mock-data'

const mascotAssets = {
  brand: '/mascot/fox_sit.png',
  idle: '/mascot/fox_sit.png',
  rest: '/mascot/fox_rest.png',
  sleep: '/mascot/fox_sleep.png',
  work: '/mascot/fox_surf.png',
  welcome: '/mascot/fox_magic.png',
  search: '/mascot/fox_search.png',
  laptop: '/mascot/fox_laptop.png',
  wrench: '/mascot/fox_wrench.png',
  cheer: '/mascot/fox_cheer.png',
  offline: '/mascot/fox_rest.png'
} as const

const mascot = mascotAssets.idle
const layoutStorage = {
  sidebarCollapsed: 'fox.layout.sidebarCollapsed',
  sidebarWidth: 'fox.layout.sidebarWidth',
  rightMode: 'fox.layout.rightMode',
  rightWidth: 'fox.layout.rightWidth',
  theme: 'fox.theme',
  focusMode: 'fox.focusMode',
  terminalHeight: 'fox.layout.terminalHeight'
} as const

const sidebarMascots = [mascotAssets.idle, mascotAssets.rest, mascotAssets.sleep]

const terminalOutput = '\u001b[36mPS D:\\python\\projects\\Fox>\u001b[0m pnpm tauri dev\n\u001b[90mFox desktop is running on http://127.0.0.1:1421\u001b[0m'
const citationUrls = [
  'https://github.com/xingyuv/kun/blob/develop/LICENSE',
  'https://github.com/xingyuv/kun/blob/develop/CLA.md'
]

const licenseAnswerMarkdown = `### 企业内部使用

营利性企业将 Kun 用作内部效率工具、业务系统或其他商业运营用途，通常属于商业使用，**需要向版权持有者申请单独的书面商业授权**。

| 使用场景 | 是否允许 |
| --- | --- |
| 个人学习与非商业二次开发 | 允许 |
| 教育、公益研究 | 允许 |
| 营利企业内部业务使用 | 需要商业授权 |
| SaaS、商业产品集成或转售 | 需要商业授权 |`

type RightMode = 'todo' | 'changes' | 'browser' | 'files' | 'knowledge' | 'agents'
type ChatState = 'complete' | 'running' | 'question' | 'approval' | 'error' | 'denied'

const filePreviewByPath: Record<string, { language: string; size: string; lines: string[] }> = {
  'docs/FOX_ARCHITECTURE.md': {
    language: 'Markdown',
    size: '1.2 KB',
    lines: ['# Fox 总体架构', '', 'Fox 是一个面向普通用户的通用桌面 Agent。', '', '## 总体架构', '', '- Fox Renderer', '- Tauri Local Bridge', '- Yuxi API']
  },
  'docs/requirements.md': {
    language: 'Markdown',
    size: '864 B',
    lines: ['# 第一阶段需求', '', '- 通用对话', '- 本地文件读写', '- Yuxi 知识库检索', '- 来源引用', '', '暂不包含定时任务。']
  },
  'research/market-notes.md': {
    language: 'Markdown',
    size: '2.4 KB',
    lines: ['# Agent 桌面端调研', '', '## 观察', '', '- 输入区需要承载模型、Agent 与执行设置', '- 文件预览应与会话并行工作', '- 过程区完成后默认折叠']
  },
  'research/references.pdf': {
    language: 'PDF',
    size: '3.8 MB',
    lines: ['PDF 文档预览', '', 'Yuxi 知识库与 Agent 架构参考资料', '', '第 1 页 / 共 24 页']
  },
  'README.md': {
    language: 'Markdown',
    size: '532 B',
    lines: ['# Fox', '', 'A lightweight general-purpose desktop agent.', '', 'Built with Tauri, React, shadcn/ui and AI Elements.']
  }
}

function readStoredBoolean(key: string, fallback: boolean) {
  if (typeof window === 'undefined') return fallback
  const value = window.localStorage.getItem(key)
  return value === null ? fallback : value === '1'
}

function readStoredNumber(key: string, fallback: number, min: number, max: number) {
  if (typeof window === 'undefined') return fallback
  const value = Number(window.localStorage.getItem(key))
  return Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback
}

function readStoredRightMode(): RightMode | null {
  if (typeof window === 'undefined') return 'files'
  const value = window.localStorage.getItem(layoutStorage.rightMode)
  return value === 'todo' || value === 'changes' || value === 'browser' || value === 'files' || value === 'knowledge' || value === 'agents'
    ? value
    : value === 'closed' ? null : 'files'
}

function IconButton({
  label,
  children,
  onClick,
  active,
  className = '',
  tooltipSide = 'top'
}: {
  label: string
  children: ReactNode
  onClick?: () => void
  active?: boolean
  className?: string
  tooltipSide?: 'top' | 'right' | 'bottom' | 'left'
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={label}
          aria-pressed={active === undefined ? undefined : active}
          className={`fox-icon-button ${active ? 'is-active' : ''} ${className}`}
          onClick={onClick}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent side={tooltipSide}><p>{label}</p></TooltipContent>
    </Tooltip>
  )
}

function WorkMascot({ busy = false, size = 'sm' }: { busy?: boolean; size?: 'sm' | 'md' }) {
  return (
    <span className={`fox-work-mascot is-${size} ${busy ? 'is-busy' : ''}`} aria-hidden="true">
      <span className="fox-work-current" />
      <span className="fox-work-wave is-back" />
      <span className="fox-work-track">
        <span className="fox-work-body">
          <img src={busy ? mascotAssets.work : mascot} alt="" />
        </span>
      </span>
      <span className="fox-work-wave is-front" />
      <span className="fox-work-foam" />
      <span className="fox-work-bubbles" />
    </span>
  )
}

function WindowTitlebar() {
  const windowAction = async (action: 'minimize' | 'maximize' | 'close') => {
    try {
      const appWindow = getCurrentWindow()
      if (action === 'minimize') await appWindow.minimize()
      if (action === 'maximize') await appWindow.toggleMaximize()
      if (action === 'close') await appWindow.close()
    } catch {
      // Browser preview has no native window; buttons remain harmless there.
    }
  }

  return (
    <header className="fox-window-titlebar" data-tauri-drag-region>
      <div className="fox-window-product" data-tauri-drag-region>
        <img src={mascotAssets.brand} alt="" />
        <strong>Fox</strong>
        <nav aria-label="应用菜单">
          <WindowMenu label="文件">
            <DropdownMenuItem><MessageSquarePlus />新建对话<DropdownMenuShortcut>Ctrl N</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem><FolderOpen />打开项目<DropdownMenuShortcut>Ctrl O</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem><Settings />设置<DropdownMenuShortcut>Ctrl ,</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => void windowAction('close')}>退出</DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="编辑">
            <DropdownMenuItem>撤销<DropdownMenuShortcut>Ctrl Z</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem>重做<DropdownMenuShortcut>Ctrl Y</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem>复制<DropdownMenuShortcut>Ctrl C</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem>粘贴<DropdownMenuShortcut>Ctrl V</DropdownMenuShortcut></DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="视图">
            <DropdownMenuItem><PanelLeftClose />切换侧边栏<DropdownMenuShortcut>Ctrl B</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem><Terminal />切换终端<DropdownMenuShortcut>Ctrl J</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem>放大<DropdownMenuShortcut>Ctrl +</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem>缩小<DropdownMenuShortcut>Ctrl -</DropdownMenuShortcut></DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="帮助">
            <DropdownMenuItem><BookOpen />Fox 文档</DropdownMenuItem>
            <DropdownMenuItem><Sparkles />关于 Fox</DropdownMenuItem>
          </WindowMenu>
        </nav>
      </div>
      <div className="fox-window-controls">
        <button type="button" aria-label="最小化" onClick={() => void windowAction('minimize')}><Minus /></button>
        <button type="button" aria-label="最大化" onClick={() => void windowAction('maximize')}><Square /></button>
        <button type="button" className="is-close" aria-label="关闭" onClick={() => void windowAction('close')}><X /></button>
      </div>
    </header>
  )
}

function WindowMenu({ label, children }: { label: string; children: ReactNode }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild><button type="button">{label}</button></DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={3} className="fox-window-menu-content">{children}</DropdownMenuContent>
    </DropdownMenu>
  )
}

function SidebarThreadRow({
  title,
  active = false,
  time
}: {
  title: string
  active?: boolean
  time?: string
}) {
  return (
    <div className={`fox-sidebar-tree-row ${active ? 'is-active' : ''}`}>
      <button type="button" className="fox-sidebar-tree-main">
        <FileText size={14} />
        <span>{title}</span>
      </button>
      <span className="fox-sidebar-row-time">{time}</span>
      <span className="fox-sidebar-row-actions">
        <IconButton label="置顶会话"><Pin size={12} /></IconButton>
        <IconButton label="归档会话"><Archive size={12} /></IconButton>
        <IconButton label="删除会话"><Trash2 size={12} /></IconButton>
      </span>
    </div>
  )
}

function Sidebar({
  collapsed,
  focusMode,
  mascotIndex,
  onCollapse,
  onFocusMode,
  onNewChat,
  onOpenConversation,
  onTheme,
  onSettings
}: {
  collapsed: boolean
  focusMode: boolean
  mascotIndex: number
  onCollapse: () => void
  onFocusMode: (value: boolean) => void
  onNewChat: () => void
  onOpenConversation: () => void
  onTheme: () => void
  onSettings: () => void
}) {
  const [projectOpen, setProjectOpen] = useState(true)
  const [search, setSearch] = useState('')
  const [modeTab, setModeTab] = useState('agent')
  const searchInputRef = useRef<HTMLInputElement | null>(null)
  const visibleConversations = conversations.filter((item) =>
    item.title.toLowerCase().includes(search.toLowerCase())
  )

  return (
    <aside className={`fox-sidebar ${collapsed ? 'is-collapsed' : ''}`}>
      <div className="fox-sidebar-title-safe">
        <span className="fox-window-safe" />
        <IconButton label={collapsed ? '展开侧边栏' : '收起侧边栏'} tooltipSide="bottom" onClick={onCollapse}>
          {collapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
        </IconButton>
      </div>

      {!collapsed && (
        <Tabs value={modeTab} onValueChange={setModeTab} className="fox-mode-tabs">
          <TabsList>
            <TabsTrigger value="agent"><Sparkles />助手</TabsTrigger>
            <TabsTrigger value="knowledge"><Library />知识</TabsTrigger>
          </TabsList>
        </Tabs>
      )}

      <div className="fox-sidebar-primary">
        <button className="fox-sidebar-command is-emphasis" onClick={onNewChat}><MessageSquarePlus size={16} /><span>新建对话</span><kbd>Ctrl N</kbd></button>
        <button className="fox-sidebar-command" onClick={() => searchInputRef.current?.focus()}><Search size={16} /><span>搜索</span><kbd>Ctrl K</kbd></button>
        <button className="fox-sidebar-command" onClick={() => setModeTab('knowledge')}><Library size={16} /><span>知识库</span></button>
      </div>

      {!collapsed && (
        <ScrollArea className="fox-sidebar-scroll">
          <section className="fox-sidebar-section">
            <div className="fox-section-head"><span>项目</span><span><IconButton label="添加项目"><Plus size={14} /></IconButton><IconButton label="更多项目操作"><MoreHorizontal size={14} /></IconButton></span></div>
            <button className="fox-project-row is-active" onClick={() => setProjectOpen(!projectOpen)}>
              {projectOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
              <FolderOpen size={15} />
              <strong>Fox</strong>
              <small>2</small>
            </button>
            {projectOpen && (
              <div className="fox-project-children">
                <div onClick={onOpenConversation}><SidebarThreadRow title="Kun 协议与企业使用" active time="刚刚" /></div>
                <SidebarThreadRow title="整理产品需求文档" time="10:42" />
                <button className="fox-project-action"><FilePlus2 size={14} />在 Fox 中新建对话</button>
              </div>
            )}
          </section>

          <section className="fox-sidebar-section fox-conversation-section">
            <div className="fox-section-head"><span>对话</span><span><IconButton label="新建会话"><Plus size={14} /></IconButton><IconButton label="归档会话"><Archive size={14} /></IconButton></span></div>
            <label className="fox-sidebar-search">
              <Search size={14} />
              <input ref={searchInputRef} value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索对话" />
              {search && <button type="button" onClick={() => setSearch('')} aria-label="清除"><X size={13} /></button>}
            </label>
            <div className="fox-conversation-list">
              {visibleConversations.map((item) => (
                <div key={item.id} className={`fox-sidebar-tree-row ${item.active ? 'is-active' : ''}`}>
                  <button type="button" className="fox-sidebar-tree-main" onClick={onOpenConversation}><MessageCircleMore size={14} /><span>{item.title}</span></button>
                  <span className="fox-sidebar-row-time">{item.time}</span>
                  <span className="fox-sidebar-row-actions"><IconButton label="更多会话操作"><MoreHorizontal size={13} /></IconButton></span>
                </div>
              ))}
            </div>
          </section>
        </ScrollArea>
      )}

      <div className="fox-sidebar-spacer" />
      {!collapsed && <div className={`fox-sidebar-character ${focusMode ? 'is-focus' : ''}`}>{!focusMode && <img key={sidebarMascots[mascotIndex]} src={sidebarMascots[mascotIndex]} alt="" />}<div className="fox-focus-toggle"><span><strong>专注模式</strong><small>{focusMode ? '已开启' : '保持当前任务'}</small></span><Switch checked={focusMode} onCheckedChange={onFocusMode} aria-label="专注模式" /></div></div>}
      <div className="fox-sidebar-footer">
        <button className="fox-profile"><Avatar size="sm"><AvatarFallback>H</AvatarFallback></Avatar><span><strong>Hury</strong><small><i />Yuxi 已连接</small></span></button>
        <IconButton label="切换主题" onClick={onTheme}><SunMoon size={16} /></IconButton>
        <IconButton label="设置" onClick={onSettings}><Settings size={16} /></IconButton>
      </div>
    </aside>
  )
}

function AssistantProcess({ running = false }: { running?: boolean }) {
  const [open, setOpen] = useState(running)

  useEffect(() => {
    setOpen(running)
  }, [running])

  return (
    <ChainOfThought open={open} onOpenChange={setOpen} className="fox-chain-of-thought">
      <ChainOfThoughtHeader className="fox-chain-of-thought-header">
        <span className={running ? 'fox-shiny-text' : ''}>{running ? '正在处理' : '工作过程（7 步）'}</span>
        {!running && <small>· 思考 10 秒</small>}
      </ChainOfThoughtHeader>
      <ChainOfThoughtContent className="fox-chain-of-thought-content">
        <ChainOfThoughtStep icon={Search} label="查找 LICENSE*" status="complete">
          <ChainOfThoughtSearchResults>
            <ChainOfThoughtSearchResult>Kun/LICENSE</ChainOfThoughtSearchResult>
          </ChainOfThoughtSearchResults>
        </ChainOfThoughtStep>
        <ChainOfThoughtStep icon={Search} label="搜索 license、License、LICENSE" status="complete">
          <ChainOfThoughtSearchResults>
            <ChainOfThoughtSearchResult>Kun/CLA.md</ChainOfThoughtSearchResult>
            <ChainOfThoughtSearchResult>Kun/README.md</ChainOfThoughtSearchResult>
          </ChainOfThoughtSearchResults>
        </ChainOfThoughtStep>
        <ChainOfThoughtStep
          icon={Brain}
          label="分析许可证问题"
          description="需要依据仓库中的 LICENSE 与 CLA 判断二次开发和企业内部使用范围。"
          status="complete"
        />
        <ChainOfThoughtStep
          icon={MessageCircleMore}
          label="好的，我来查一下 Fox 项目（Kun）的许可证信息。"
          status="complete"
        />
        <ChainOfThoughtStep icon={FileText} label="读取 Kun/LICENSE" status="complete">
          <div className="fox-chain-detail">确认项目采用 PolyForm Noncommercial License 1.0.0。</div>
        </ChainOfThoughtStep>
        <ChainOfThoughtStep
          icon={Brain}
          label="核对许可范围"
          description="非商业学习、研究和实验允许使用；商业用途需要单独书面授权。"
          status="complete"
        />
        <ChainOfThoughtStep
          icon={ShieldCheck}
          label={running ? '正在判断企业内部使用场景…' : '判断企业内部使用场景'}
          description="营利性企业用于内部业务通常属于商业用途，需要向版权持有者申请授权。"
          status={running ? 'active' : 'complete'}
        />
      </ChainOfThoughtContent>
    </ChainOfThought>
  )
}

function ApprovalPrompt({ onApprove, onDeny }: { onApprove: () => void; onDeny: () => void }) {
  return (
    <Confirmation approval={{ id: 'fox-write-file' }} state="approval-requested" className="fox-confirmation">
      <ConfirmationRequest>
        <div className="fox-confirmation-body">
          <span className="fox-prompt-figure"><img src={mascotAssets.wrench} alt="" /></span>
          <div><ConfirmationTitle>允许 Fox 修改本地文件？</ConfirmationTitle><p><code>apps/desktop/src/styles/workbench.css</code></p><small>Fox 将写入此工作区中的一个文件。你可以在 Changes 面板检查修改。</small></div>
        </div>
        <ConfirmationActions className="fox-confirmation-actions">
          <ConfirmationAction variant="ghost" onClick={onDeny}>拒绝</ConfirmationAction>
          <ConfirmationAction onClick={onApprove}>允许一次</ConfirmationAction>
        </ConfirmationActions>
      </ConfirmationRequest>
    </Confirmation>
  )
}

function DeniedPrompt() {
  return (
    <Confirmation approval={{ id: 'fox-write-file', approved: false }} state="approval-responded" className="fox-denied-confirmation">
      <ConfirmationRejected>
        <div className="fox-denied-prompt">
          <img className="fox-inline-mascot" src={mascotAssets.cheer} alt="" />
          <span><strong>操作已取消</strong><small>Fox 没有修改任何本地文件。</small></span>
        </div>
      </ConfirmationRejected>
    </Confirmation>
  )
}

function ErrorPrompt({ onRetry }: { onRetry: () => void }) {
  return (
    <div className="fox-error-prompt" role="alert">
      <span className="fox-prompt-figure"><img src={mascotAssets.offline} alt="" /></span>
      <div><strong>无法连接到 Yuxi 服务</strong><p>本地服务暂时没有响应。请确认后端已启动，或稍后重试。</p><small>ECONNREFUSED · 127.0.0.1:5010</small></div>
      <Button size="sm" variant="outline" onClick={onRetry}><RotateCcw size={14} />重试</Button>
    </div>
  )
}

const clarificationOptions = [
  { id: 'analyze', title: '仅分析', detail: '先阅读相关文件并给出建议，不写入工作区' },
  { id: 'edit', title: '直接修改', detail: '按推荐方案修改文件，写入前仍会请求授权' },
  { id: 'backup', title: '先备份再修改', detail: '保留原文件副本后再执行修改' }
] as const

function QuestionPrompt({ onAnswer }: { onAnswer: (answer: string) => void }) {
  return (
    <div className="fox-question-prompt">
      <div className="fox-question-heading">
        <span className="fox-prompt-figure"><img src={mascotAssets.search} alt="" /></span>
        <div><strong>你希望我怎样处理这些文件？</strong><p>确认执行方式后，Fox 会继续当前任务。</p></div>
      </div>
      <div className="fox-question-options">
        {clarificationOptions.map((option, index) => (
          <button type="button" key={option.id} onClick={() => onAnswer(option.title)}>
            <kbd>{index + 1}</kbd><span><strong>{option.title}</strong><small>{option.detail}</small></span><ChevronRight size={14} />
          </button>
        ))}
      </div>
    </div>
  )
}

function LicenseInlineCitation() {
  return (
    <InlineCitation className="fox-inline-citation">
      <InlineCitationText>Kun 使用 PolyForm Noncommercial License 1.0.0</InlineCitationText>
      <InlineCitationCard>
        <InlineCitationCardTrigger sources={citationUrls} className="fox-inline-citation-trigger">2 个依据</InlineCitationCardTrigger>
        <InlineCitationCardBody className="fox-inline-citation-card">
          <InlineCitationCarousel>
            <InlineCitationCarouselHeader className="fox-inline-citation-head">
              <InlineCitationCarouselPrev />
              <InlineCitationCarouselIndex />
              <InlineCitationCarouselNext />
            </InlineCitationCarouselHeader>
            <InlineCitationCarouselContent>
              <InlineCitationCarouselItem>
                <InlineCitationSource title="PolyForm Noncommercial License 1.0.0" url="Kun/LICENSE" description="项目当前使用的非商业许可协议。" />
                <InlineCitationQuote>许可范围涵盖个人、教育、研究及其他非商业用途。</InlineCitationQuote>
              </InlineCitationCarouselItem>
              <InlineCitationCarouselItem>
                <InlineCitationSource title="Contributor License Agreement" url="Kun/CLA.md" description="确认项目授权与商业许可安排。" />
                <InlineCitationQuote>商业使用需要获得版权持有者单独的书面授权。</InlineCitationQuote>
              </InlineCitationCarouselItem>
            </InlineCitationCarouselContent>
          </InlineCitationCarousel>
        </InlineCitationCardBody>
      </InlineCitationCard>
    </InlineCitation>
  )
}

function Timeline({ empty, state, prompt, onApprove, onDeny, onRetry, onAnswer, onStart }: { empty: boolean; state: ChatState; prompt: string; onApprove: () => void; onDeny: () => void; onRetry: () => void; onAnswer: (answer: string) => void; onStart: (suggestion: string) => void }) {
  const running = state === 'running'
  const userTurnRef = useRef<HTMLDivElement | null>(null)
  const assistantTurnRef = useRef<HTMLDivElement | null>(null)
  const [hoveredTurn, setHoveredTurn] = useState<'user' | 'assistant' | null>(null)

  if (empty) {
    return (
      <Conversation className="fox-conversation">
        <ConversationEmptyState className="fox-empty-conversation">
          <img src={mascotAssets.welcome} alt="" />
          <div><h2>今天想一起做点什么？</h2><p>Fox 可以和你对话、处理本地文件，也可以从 Yuxi 知识库中检索资料。</p></div>
          <Suggestions className="fox-starter-suggestions">
            <Suggestion suggestion="整理这个项目中的文档" onClick={onStart} />
            <Suggestion suggestion="从知识库查找相关资料" onClick={onStart} />
            <Suggestion suggestion="读取一个文件并帮我分析" onClick={onStart} />
          </Suggestions>
        </ConversationEmptyState>
      </Conversation>
    )
  }
  return (
    <Conversation className="fox-conversation">
      <nav className="fox-turn-rail" aria-label="回合导航">
        <button type="button" className="is-active" aria-label="跳到最近的用户消息" onClick={() => userTurnRef.current?.scrollIntoView({ behavior: 'smooth', block: 'center' })} onMouseEnter={() => setHoveredTurn('user')} onMouseLeave={() => setHoveredTurn(null)} onFocus={() => setHoveredTurn('user')} onBlur={() => setHoveredTurn(null)}><span /><span /><span /></button>
        <button type="button" aria-label="跳到助手回复" onClick={() => assistantTurnRef.current?.scrollIntoView({ behavior: 'smooth', block: 'center' })} onMouseEnter={() => setHoveredTurn('assistant')} onMouseLeave={() => setHoveredTurn(null)} onFocus={() => setHoveredTurn('assistant')} onBlur={() => setHoveredTurn(null)}><span /><span /></button>
        <div className={`fox-turn-preview ${hoveredTurn ? 'is-visible' : ''}`}><strong>{hoveredTurn === 'assistant' ? 'Fox 的回复' : '你的问题'}</strong><span>{hoveredTurn === 'assistant' ? '确认 UI、Tauri Bridge 与 Yuxi 的职责边界' : prompt}</span></div>
      </nav>
      <ConversationContent className="fox-conversation-content">
        <div className="fox-thread-date"><span>今天</span></div>
        <div ref={userTurnRef} className="fox-turn-anchor">
        <Message from="user" className="fox-message fox-user-message">
          <MessageContent className="fox-user-bubble">{prompt}</MessageContent>
          <div className="fox-user-meta"><span>DeepSeek V3.2 · 刚刚</span><span><MessageAction tooltip="复制"><Copy size={13} /></MessageAction><MessageAction tooltip="编辑"><FileEdit size={13} /></MessageAction></span></div>
        </Message>
        </div>
        <div ref={assistantTurnRef} className="fox-turn-anchor">
        <Message from="assistant" className="fox-message fox-assistant-message">
          <MessageContent className="fox-assistant-content">
            {(state === 'running' || state === 'complete') && <AssistantProcess running={running} />}
            {state === 'question' ? <QuestionPrompt onAnswer={onAnswer} /> : state === 'approval' ? <ApprovalPrompt onApprove={onApprove} onDeny={onDeny} /> : state === 'error' ? <ErrorPrompt onRetry={onRetry} /> : state === 'denied' ? <DeniedPrompt /> : running ? (
              <div className="fox-live-response"><WorkMascot busy /><span className="fox-shiny-text">正在连接 Yuxi 并整理结果</span><i><b /><b /><b /></i></div>
            ) : <>
              <div className="fox-answer-body">
                <h2>Kun 的许可证</h2>
                <p><LicenseInlineCitation />，允许个人学习、研究、实验和其他非商业用途，也可以在许可范围内进行二次开发。</p>
                <MessageResponse className="fox-answer-response">{licenseAnswerMarkdown}</MessageResponse>
              </div>
              <Sources defaultOpen className="fox-sources">
              <SourcesTrigger count={citations.length}>引用了 {citations.length} 个来源<ChevronDown size={14} /></SourcesTrigger>
              <SourcesContent>
                {citations.map((item, index) => (
                  <Source key={item.id} href="#" title={item.title} onClick={(event) => event.preventDefault()}>
                    <span className="fox-citation-index">{index + 1}</span>
                    <span><strong>{item.title}</strong><small>{item.detail}</small></span>
                    <ChevronRight size={14} />
                  </Source>
                ))}
              </SourcesContent>
              </Sources>
            </>}
          </MessageContent>
          {state === 'complete' && <MessageActions className="fox-message-actions">
            <MessageAction tooltip="复制" onClick={() => { void navigator.clipboard?.writeText('Kun 使用 PolyForm Noncommercial License 1.0.0，企业商业使用需要单独授权。'); toast.success('已复制回复') }}><Copy size={14} /></MessageAction>
            <MessageAction tooltip="有帮助" onClick={() => toast.success('感谢反馈')}><ThumbsUp size={14} /></MessageAction>
            <MessageAction tooltip="没有帮助" onClick={() => toast('已记录反馈')}><ThumbsDown size={14} /></MessageAction>
            <MessageAction tooltip="重新生成" onClick={() => toast('重新生成将在接入 Yuxi 后启用')}><RotateCcw size={14} /></MessageAction>
          </MessageActions>}
        </Message>
        </div>
      </ConversationContent>
      <ConversationScrollButton className="fox-scroll-button" />
    </Conversation>
  )
}

function ComposerAttachmentPreview() {
  const attachments = usePromptInputAttachments()
  if (!attachments.files.length) return null
  return (
    <Attachments variant="inline" className="fox-composer-attachments">
      {attachments.files.map((file) => (
        <Attachment key={file.id} data={file} onRemove={() => attachments.remove(file.id)}>
          <AttachmentPreview />
          <AttachmentInfo />
          <AttachmentRemove label="移除附件" />
        </Attachment>
      ))}
    </Attachments>
  )
}

const slashCommands = [
  { id: 'files', title: '读取文件', detail: '从当前工作区选择文件并加入上下文', icon: <FileText size={15} /> },
  { id: 'knowledge', title: '搜索知识库', detail: '使用 Yuxi RAG 检索相关资料', icon: <Library size={15} /> },
  { id: 'plan', title: '规划任务', detail: '先生成执行计划，再开始处理', icon: <ListTodo size={15} /> },
  { id: 'summarize', title: '总结对话', detail: '压缩当前会话并保留关键上下文', icon: <Sparkles size={15} /> }
]

const goalStateCopy: Record<ChatState, { label: string; detail: string; time: string }> = {
  complete: { label: '已完成', detail: '确认 Kun 授权范围与 Fox 开发边界', time: '刚刚' },
  running: { label: '进行中', detail: '正在整理当前问题的依据与结论', time: '12 分钟' },
  question: { label: '等待回答', detail: '需要你选择文件处理方式后继续', time: '待你确认' },
  approval: { label: '等待批准', detail: '需要授权后才能写入本地工作区', time: '待你批准' },
  error: { label: '连接错误', detail: 'Yuxi 服务暂时无响应，可重试当前步骤', time: '需处理' },
  denied: { label: '已取消', detail: '已取消本次写入，工作区没有变更', time: '刚刚' }
}

function Composer({ resetKey, suggestedPrompt, chatState, onSubmitPrompt, onStatusChange }: { resetKey: number; suggestedPrompt?: string; chatState: ChatState; onSubmitPrompt?: (prompt: string) => 'complete' | 'question' | 'approval' | 'error' | void; onStatusChange?: (status: 'ready' | 'streaming') => void }) {
  const [status, setStatus] = useState<'ready' | 'streaming'>('ready')
  const [focused, setFocused] = useState(false)
  const [model, setModel] = useState('deepseek-v3.2')
  const [agent, setAgent] = useState('general')
  const [execution, setExecution] = useState('ask')
  const [goalPaused, setGoalPaused] = useState(false)
  const [draft, setDraft] = useState('')
  const [commandOpen, setCommandOpen] = useState(false)
  const [mentionOpen, setMentionOpen] = useState(false)
  const submitTimer = useRef<number | null>(null)

  useEffect(() => () => {
    if (submitTimer.current !== null) window.clearTimeout(submitTimer.current)
  }, [])

  useEffect(() => {
    if (submitTimer.current !== null) {
      window.clearTimeout(submitTimer.current)
      submitTimer.current = null
    }
    setStatus('ready')
    setDraft(suggestedPrompt ?? '')
    setCommandOpen(false)
    setMentionOpen(false)
  }, [resetKey, suggestedPrompt])

  const onDraftChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    const value = event.currentTarget.value
    setDraft(value)
    setCommandOpen(value.startsWith('/'))
    setMentionOpen(!value.startsWith('/') && /(^|\s)@[\w./-]*$/.test(value))
  }

  const applySlashCommand = (command: typeof slashCommands[number]) => {
    const next = command.id === 'files' ? '请读取并分析 @docs/FOX_ARCHITECTURE.md' : command.id === 'knowledge' ? '请从知识库搜索 Fox 架构相关资料' : command.id === 'plan' ? '请先规划接下来的开发步骤' : '请总结当前对话'
    setDraft(next)
    setCommandOpen(false)
  }

  const applyMention = (path: string) => {
    setDraft((value) => value.replace(/@[\w./-]*$/, `@${path} `))
    setMentionOpen(false)
  }

  const goalState = goalPaused ? { label: '已暂停', detail: '目标已暂停，继续后会回到当前任务', time: '暂停中' } : goalStateCopy[chatState]

  return (
    <div className="fox-composer-wrap">
      <div className={`fox-goal-floater is-${goalPaused ? 'paused' : chatState}`}>
        <span className="fox-goal-icon"><Target size={15} /></span>
        <span className="fox-goal-copy">
          <span><strong>当前目标</strong><Badge>{goalState.label}</Badge></span>
          <small>{goalState.detail}</small>
        </span>
        <span className="fox-goal-time">{goalState.time}</span>
        <span className="fox-goal-actions">
          <IconButton label="编辑目标"><FileEdit size={13} /></IconButton>
          <IconButton label={goalPaused ? '继续目标' : '暂停目标'} onClick={() => setGoalPaused(!goalPaused)}>{goalPaused ? <Play size={13} /> : <Pause size={13} />}</IconButton>
          <IconButton label="清除目标"><Trash2 size={13} /></IconButton>
        </span>
      </div>
      <PromptInput
        accept="image/*,.pdf,.txt,.md,.doc,.docx"
        multiple
        maxFiles={8}
        onSubmit={({ text, files: submittedFiles }) => {
          if (!text.trim() && submittedFiles.length === 0) return
          setStatus('streaming')
          setDraft('')
          setCommandOpen(false)
          setMentionOpen(false)
          const outcome = onSubmitPrompt?.(text.trim() || '请分析这些附件')
          if (outcome === 'question' || outcome === 'approval' || outcome === 'error') {
            setStatus('ready')
            onStatusChange?.('ready')
            return
          }
          onStatusChange?.('streaming')
          if (submitTimer.current !== null) window.clearTimeout(submitTimer.current)
          submitTimer.current = window.setTimeout(() => {
            setStatus('ready')
            onStatusChange?.('ready')
            submitTimer.current = null
          }, 3600)
        }}
        className={`fox-prompt-input ${focused ? 'is-focused' : ''}`}
      >
        {commandOpen && <Command className="fox-slash-menu">
          <div className="fox-command-title"><span>命令</span><kbd>Esc</kbd></div>
          <CommandList>
            <CommandEmpty>没有匹配的命令</CommandEmpty>
            <CommandGroup>
              {slashCommands.map((command) => <CommandItem key={command.id} value={command.id} onSelect={() => applySlashCommand(command)}><span className="fox-command-icon">{command.icon}</span><span><strong>/{command.id}</strong><small>{command.title} · {command.detail}</small></span><kbd>↵</kbd></CommandItem>)}
            </CommandGroup>
          </CommandList>
        </Command>}
        {mentionOpen && <div className="fox-mention-menu"><div className="fox-command-title"><span>引用工作区文件</span><kbd>@</kbd></div>{['docs/FOX_ARCHITECTURE.md', 'apps/desktop/src/features/chat/workbench.tsx', 'apps/desktop/src/styles/workbench.css'].map((path) => <button type="button" key={path} onMouseDown={(event) => event.preventDefault()} onClick={() => applyMention(path)}><FileText size={14} /><span>{path}</span></button>)}</div>}
        <ComposerAttachmentPreview />
        <div className="fox-composer-context"><span><FolderOpen size={13} />Fox<X size={12} /></span><span><Library size={13} />产品知识库<X size={12} /></span></div>
        <PromptInputBody>
          <PromptInputTextarea value={draft} onChange={onDraftChange} onKeyDown={(event: KeyboardEvent<HTMLTextAreaElement>) => { if (event.key === 'Escape') { setCommandOpen(false); setMentionOpen(false) } }} className="fox-prompt-textarea" placeholder="给 Fox 发消息，输入 / 查看命令，@ 引用文件…" onFocus={() => setFocused(true)} onBlur={() => setFocused(false)} />
        </PromptInputBody>
        <PromptInputFooter className="fox-prompt-toolbar">
          <PromptInputTools className="fox-composer-tools">
            <PromptInputActionMenu>
              <PromptInputActionMenuTrigger tooltip="添加内容"><Plus size={18} /></PromptInputActionMenuTrigger>
              <PromptInputActionMenuContent className="fox-add-menu">
                <PromptInputActionAddAttachments label="添加文件" />
                <PromptInputActionMenuItem><ImagePlus />添加图片</PromptInputActionMenuItem>
                <PromptInputActionMenuItem><Library />选择知识库</PromptInputActionMenuItem>
              </PromptInputActionMenuContent>
            </PromptInputActionMenu>
            <PromptInputSelect value={execution} onValueChange={setExecution}>
              <PromptInputSelectTrigger className="fox-execution-control"><Terminal size={14} /><PromptInputSelectValue /></PromptInputSelectTrigger>
              <PromptInputSelectContent>
                <PromptInputSelectItem value="ask">执行前询问</PromptInputSelectItem>
                <PromptInputSelectItem value="auto">自动执行</PromptInputSelectItem>
                <PromptInputSelectItem value="read-only">只读模式</PromptInputSelectItem>
              </PromptInputSelectContent>
            </PromptInputSelect>
          </PromptInputTools>
          <div className="fox-composer-submit">
            <AIContext usedTokens={8400} maxTokens={23000}>
              <AIContextTrigger>
                <button type="button" className="fox-context-capacity" aria-label="上下文已使用 36%"><span /></button>
              </AIContextTrigger>
              <AIContextContent side="top" align="end" sideOffset={10} className="fox-context-popover">
                <AIContextContentHeader />
                <AIContextContentBody className="fox-context-breakdown">
                  <div><span>输入</span><strong>7.2k tokens</strong></div>
                  <div><span>输出</span><strong>1.2k tokens</strong></div>
                  <div><span>可用</span><strong>14.6k tokens</strong></div>
                </AIContextContentBody>
                <AIContextContentFooter className="fox-context-popover-footer">
                  <span>DeepSeek V3.2</span><span>36%</span>
                </AIContextContentFooter>
              </AIContextContent>
            </AIContext>
            <PromptInputSelect value={model} onValueChange={setModel}>
              <PromptInputSelectTrigger className="fox-model-control"><PromptInputSelectValue /></PromptInputSelectTrigger>
              <PromptInputSelectContent>
                <PromptInputSelectItem value="deepseek-v3.2">DeepSeek V3.2</PromptInputSelectItem>
                <PromptInputSelectItem value="gpt-5.4">GPT-5.4</PromptInputSelectItem>
                <PromptInputSelectItem value="qwen-max">Qwen Max</PromptInputSelectItem>
              </PromptInputSelectContent>
            </PromptInputSelect>
            <PromptInputSelect value={agent} onValueChange={setAgent}>
              <PromptInputSelectTrigger className="fox-agent-control"><Sparkles size={14} /><PromptInputSelectValue /></PromptInputSelectTrigger>
              <PromptInputSelectContent>
                <PromptInputSelectItem value="general">通用助手</PromptInputSelectItem>
                <PromptInputSelectItem value="knowledge">知识助手</PromptInputSelectItem>
                <PromptInputSelectItem value="files">文件助手</PromptInputSelectItem>
              </PromptInputSelectContent>
            </PromptInputSelect>
            <PromptInputButton className="fox-voice-button" tooltip="语音输入"><Mic size={17} /></PromptInputButton>
            <PromptInputSubmit
              status={status}
              onStop={() => {
                if (submitTimer.current !== null) {
                  window.clearTimeout(submitTimer.current)
                  submitTimer.current = null
                }
                setStatus('ready')
                onStatusChange?.('ready')
              }}
              className="fox-send-button"
            >
              {status === 'streaming' ? <CircleStop size={17} /> : <ArrowUp size={18} />}
            </PromptInputSubmit>
          </div>
        </PromptInputFooter>
      </PromptInput>
      <div className="fox-composer-meta"><span><i />Fox 项目 · Yuxi 本地服务</span><span>8.4k tokens · 上下文 36%</span><span>Fox 可能会出错，请核对重要信息。</span></div>
    </div>
  )
}

function ContextContent({ mode }: { mode: RightMode }) {
  const [selectedFile, setSelectedFile] = useState('docs/FOX_ARCHITECTURE.md')
  const [selectedChange, setSelectedChange] = useState('workbench.tsx')
  const [fileSearch, setFileSearch] = useState('')
  if (mode === 'files') {
    const preview = filePreviewByPath[selectedFile] ?? filePreviewByPath['README.md']
    const previewLines = preview.lines
    const breadcrumbs = selectedFile.split('/')
    const normalizedSearch = fileSearch.trim().toLowerCase()
    const visibleFiles = files.flatMap((item) => item.type === 'folder'
      ? (item.children ?? []).map((child) => ({ path: `${item.name}/${child}`, name: child, folder: item.name }))
      : [{ path: item.name, name: item.name, folder: '' }]
    ).filter((item) => !normalizedSearch || item.path.toLowerCase().includes(normalizedSearch))
    return (
      <div className="fox-file-panel">
        <div className="fox-file-tree-pane">
          <div className="fox-file-tree-tabs"><button type="button" className="is-active">工作区</button><button type="button">设计</button><IconButton label="更多文件操作"><MoreHorizontal size={14} /></IconButton></div>
          <div className="fox-panel-search"><Search size={14} /><input value={fileSearch} onChange={(event) => setFileSearch(event.target.value)} placeholder="搜索文件" />{fileSearch && <button type="button" aria-label="清除搜索" onClick={() => setFileSearch('')}><X size={12} /></button>}</div>
          <ScrollArea className="fox-file-tree">
            {normalizedSearch ? <div className="fox-file-search-results">{visibleFiles.map((item) => <button type="button" key={item.path} className={selectedFile === item.path ? 'is-active' : ''} onClick={() => setSelectedFile(item.path)}><FileText size={14} /><span><strong>{item.name}</strong><small>{item.folder || 'Fox'}</small></span></button>)}{visibleFiles.length === 0 && <p>没有匹配的文件</p>}</div> : <FileTree defaultExpanded={new Set(['docs', 'research'])} selectedPath={selectedFile} onSelect={setSelectedFile} className="fox-ai-file-tree">
              {files.map((item) => item.type === 'folder' ? (
                <FileTreeFolder key={item.name} path={item.name} name={item.name}>
                  {item.children?.map((child) => <FileTreeFile key={child} path={`${item.name}/${child}`} name={child} />)}
                </FileTreeFolder>
              ) : <FileTreeFile key={item.name} path={item.name} name={item.name} />)}
            </FileTree>}
          </ScrollArea>
        </div>
        <Artifact className="fox-file-preview">
          <ArtifactHeader className="fox-file-artifact-head">
            <div className="fox-file-artifact-title"><FileText size={14} /><div><ArtifactTitle>{selectedFile.split('/').at(-1)}</ArtifactTitle><ArtifactDescription>{breadcrumbs.join(' / ')} · {preview.size}</ArtifactDescription></div></div>
            <ArtifactActions><ArtifactAction icon={ExternalLink} tooltip="在编辑器中打开" onClick={() => toast('编辑器桥接将在接入 Tauri 文件能力后启用')} /></ArtifactActions>
          </ArtifactHeader>
          <ArtifactContent className="fox-file-artifact-content">
            <CodeBlock code={previewLines.join('\n')} language="markdown" showLineNumbers className="fox-file-code-block">
              <CodeBlockHeader className="fox-file-code-head"><CodeBlockTitle><CodeBlockFilename>{selectedFile}</CodeBlockFilename></CodeBlockTitle><CodeBlockActions><CodeBlockCopyButton onCopy={() => toast.success('已复制文件内容')} /></CodeBlockActions></CodeBlockHeader>
            </CodeBlock>
          </ArtifactContent>
        </Artifact>
      </div>
    )
  }
  if (mode === 'knowledge') {
    return <ScrollArea className="fox-source-list">{citations.map((item, index) => <button key={item.id}><span>{index + 1}</span><div><strong>{item.title}</strong><small>{item.detail}</small></div></button>)}</ScrollArea>
  }
  if (mode === 'changes') {
    const changeFiles = [
      { name: 'workbench.tsx', added: 82, removed: 10 },
      { name: 'workbench.css', added: 42, removed: 8 }
    ]
    const diffLines = selectedChange === 'workbench.tsx'
      ? [
          ['meta', '@@ -499,7 +499,12 @@ function Composer()'],
          ['removed', "-  const [model, setModel] = useState('deepseek-v3.2')"],
          ['added', "+  const [model, setModel] = useState('deepseek-v3.2')"],
          ['added', "+  const [agent, setAgent] = useState('general')"],
          ['added', "+  const [execution, setExecution] = useState('ask')"],
          ['context', ''],
          ['context', '   return (']
        ]
      : [
          ['meta', '@@ -1299,8 +1299,14 @@ .fox-composer-wrap'],
          ['removed', '-  box-shadow: 0 18px 46px var(--fox-shadow);'],
          ['added', '+  box-shadow: 0 12px 34px var(--fox-shadow);'],
          ['added', '+  transition: border-color 150ms ease;'],
          ['context', ' }']
        ]
    return (
      <div className="fox-change-panel">
        <div className="fox-change-list">
          {changeFiles.map((item) => <button type="button" key={item.name} className={selectedChange === item.name ? 'is-active' : ''} onClick={() => setSelectedChange(item.name)}><FileEdit size={14} /><span><strong>{item.name}</strong><small><b>+{item.added}</b><em>-{item.removed}</em></small></span></button>)}
        </div>
        <div className="fox-diff-view">
          <div className="fox-diff-head"><FileEdit size={14} /><strong>{selectedChange}</strong><span><b>+{selectedChange === 'workbench.tsx' ? 82 : 42}</b><em>-{selectedChange === 'workbench.tsx' ? 10 : 8}</em></span></div>
          <ScrollArea className="fox-diff-scroll"><code>{diffLines.map(([tone, line], index) => <span key={index} className={`is-${tone}`}><i>{index + 1}</i><b>{line || ' '}</b></span>)}</code></ScrollArea>
        </div>
      </div>
    )
  }
  if (mode === 'todo') {
    return <div className="fox-todo-panel"><div className="fox-todo-stats"><span><strong>2</strong><small>待处理</small></span><span><strong>1</strong><small>进行中</small></span><span><strong>4</strong><small>已完成</small></span></div><Queue className="fox-ai-queue"><QueueList className="fox-ai-queue-list"><QueueItem className="fox-ai-queue-item"><div className="fox-ai-queue-row"><QueueItemIndicator className="is-progress" /><QueueItemContent>精细化复刻主工作台</QueueItemContent><QueueItemActions><Badge>进行中</Badge></QueueItemActions></div><QueueItemDescription>正在处理右侧面板与微交互</QueueItemDescription></QueueItem><QueueItem className="fox-ai-queue-item"><div className="fox-ai-queue-row"><QueueItemIndicator /><QueueItemContent>连接 Yuxi 会话接口</QueueItemContent></div><QueueItemDescription>UI 验收后进入后端适配</QueueItemDescription></QueueItem><QueueItem className="fox-ai-queue-item"><div className="fox-ai-queue-row"><QueueItemIndicator completed /><QueueItemContent completed>替换 Fox 吉祥物素材</QueueItemContent></div><QueueItemDescription completed>已接入 Fox 全套动作素材</QueueItemDescription></QueueItem></QueueList></Queue></div>
  }
  if (mode === 'browser') {
    return <WebPreview defaultUrl="https://fox.local/preview" className="fox-browser-panel"><div className="fox-browser-tabs"><button type="button" className="is-active"><Globe2 size={13} />预览</button><Plus size={14} /></div><WebPreviewNavigation className="fox-browser-address"><WebPreviewNavigationButton tooltip="刷新"><RotateCcw size={13} /></WebPreviewNavigationButton><WebPreviewUrl aria-label="预览地址" /><WebPreviewNavigationButton tooltip="在浏览器中打开" onClick={() => toast('浏览器能力将在后端接入阶段启用')}><ExternalLink size={13} /></WebPreviewNavigationButton></WebPreviewNavigation><div className="fox-browser-empty"><img className="fox-panel-mascot" src={mascotAssets.search} alt="" /><strong>浏览器预览尚未启动</strong><span>接入网页工具后，这里会显示 Agent 正在访问的页面。</span><Button size="sm" variant="outline" onClick={() => toast('浏览器能力将在后端接入阶段启用')}>启动预览</Button></div></WebPreview>
  }
  if (mode === 'agents') {
    return <AIAgent className="fox-agent-panel"><div className="fox-agent-summary"><div className="fox-agent-avatars"><span><img src={mascotAssets.laptop} alt="" /></span><span><img src={mascotAssets.search} alt="" /></span><span><Check size={11} /></span></div><div className="fox-agent-heading"><AIAgentHeader name="子 Agent" model="Yuxi" /><small>0 个运行中 · 1 个已完成</small></div></div><AIAgentContent className="fox-agent-content"><button type="button" className="fox-agent-create" onClick={() => toast('SubAgent 将复用 Yuxi 的编排能力')}><Plus size={15} />新建子任务</button><div className="fox-agent-section-label"><span>最近任务</span><small>1</small></div><button type="button" className="fox-agent-history" onClick={() => toast('子任务详情将在接入 Yuxi 后显示')}><span className="fox-agent-state"><Check size={12} /></span><p><strong>审查 Kun 工作台结构</strong><small>UI 结构、组件复用与响应式检查</small></p><span className="fox-agent-metrics"><strong>3 分钟</strong><small>7 步</small></span><ChevronRight size={14} /></button></AIAgentContent></AIAgent>
  }
  const labels: Record<Exclude<RightMode, 'files' | 'knowledge' | 'changes'>, [string, string]> = {
    todo: ['暂无待办', 'Fox 创建的待办会显示在这里'],
    browser: ['浏览器未启动', '网页预览会显示在这里'],
    agents: ['暂无子 Agent', '后续阶段将接入 Yuxi SubAgents']
  }
  return <div className="fox-empty-panel"><ClipboardList size={29} /><strong>{labels[mode][0]}</strong><span>{labels[mode][1]}</span></div>
}

const rightItems: Array<[RightMode, string, ReactNode]> = [
  ['knowledge', '知识库', <BookOpen size={16} />],
  ['files', '文件', <Folders size={16} />],
  ['changes', '更改', <FileEdit size={16} />],
  ['todo', '待办', <ListTodo size={16} />],
  ['browser', '浏览器', <Globe2 size={16} />],
  ['agents', '子 Agent', <Bot size={16} />]
]

function RightPanel({ mode, width, compact = false, onClose }: { mode: RightMode; width: number; compact?: boolean; onClose: () => void }) {
  const item = rightItems.find(([id]) => id === mode)
  const detail = mode === 'files' ? 'Fox 项目' : mode === 'changes' ? '2 个文件已更改' : '当前会话'
  return (
    <aside className={`fox-context-panel ${compact ? 'is-compact' : ''}`} style={compact ? undefined : { width }}>
      <div className="fox-context-head"><IconButton label="关闭右侧面板" onClick={onClose}><PanelRightClose size={16} /></IconButton><div><strong>{item?.[1]}</strong><small>{detail}</small></div></div>
      <ContextContent mode={mode} />
    </aside>
  )
}

function SideRail({ mode, onMode }: { mode: RightMode | null; onMode: (mode: RightMode) => void }) {
  return (
    <nav className="fox-side-rail" aria-label="工作台工具">
      {rightItems.map(([id, label, icon]) => <div key={id} className="fox-rail-slot"><IconButton label={label} tooltipSide="left" active={mode === id} onClick={() => onMode(id)}>{icon}</IconButton>{id === 'changes' && <span className="fox-rail-count">2</span>}</div>)}
      <span />
      <div className="fox-rail-slot"><IconButton label="设置" tooltipSide="left"><Settings size={16} /></IconButton></div>
    </nav>
  )
}

function ResizeDivider({ onResize }: { onResize: (delta: number) => void }) {
  const start = useRef<{ x: number } | null>(null)
  return (
    <div
      className="fox-resize-divider"
      role="separator"
      aria-label="调整右侧面板宽度"
      aria-orientation="vertical"
      onPointerDown={(event) => { start.current = { x: event.clientX }; event.currentTarget.setPointerCapture(event.pointerId) }}
      onPointerMove={(event) => { if (!start.current) return; const delta = start.current.x - event.clientX; start.current = { x: event.clientX }; onResize(delta) }}
      onPointerUp={() => { start.current = null }}
    ><span /></div>
  )
}

function SidebarResizeDivider({ onResize }: { onResize: (delta: number) => void }) {
  const start = useRef<{ x: number } | null>(null)
  return <div className="fox-sidebar-resize-divider" role="separator" aria-label="调整侧边栏宽度" aria-orientation="vertical" onPointerDown={(event) => { start.current = { x: event.clientX }; event.currentTarget.setPointerCapture(event.pointerId) }} onPointerMove={(event) => { if (!start.current) return; const delta = event.clientX - start.current.x; start.current = { x: event.clientX }; onResize(delta) }} onPointerUp={() => { start.current = null }}><span /></div>
}

function TerminalResizeDivider({ onResize }: { onResize: (delta: number) => void }) {
  const start = useRef<{ y: number } | null>(null)
  return <div className="fox-terminal-resize-divider" role="separator" aria-label="调整终端高度" aria-orientation="horizontal" onPointerDown={(event) => { start.current = { y: event.clientY }; event.currentTarget.setPointerCapture(event.pointerId) }} onPointerMove={(event) => { if (!start.current) return; const delta = start.current.y - event.clientY; start.current = { y: event.clientY }; onResize(delta) }} onPointerUp={() => { start.current = null }} />
}

function ChatTopbar({ collapsed, onCollapse, dark, focusMode, terminalOpen, state, onDark, onFocusMode, onTerminal }: { collapsed: boolean; onCollapse: () => void; dark: boolean; focusMode: boolean; terminalOpen: boolean; state: ChatState; onDark: () => void; onFocusMode: () => void; onTerminal: () => void }) {
  return (
    <header className="fox-chat-topbar">
      <div className="fox-workspace-heading">
        <IconButton label={collapsed ? '展开侧边栏' : '收起侧边栏'} tooltipSide="bottom" onClick={onCollapse}>{collapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}</IconButton>
        <div><strong>Kun 协议与企业使用</strong><span>Fox · agent · 刚刚</span></div>
      </div>
      <div className="fox-chat-top-actions">
        <DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="选择编辑器"><Code2 size={16} /></Button></DropdownMenuTrigger></TooltipTrigger>
            <TooltipContent side="bottom"><p>选择编辑器</p></TooltipContent>
          </Tooltip>
          <DropdownMenuContent align="end" className="fox-editor-menu">
            <DropdownMenuLabel>在编辑器中打开</DropdownMenuLabel>
            <DropdownMenuItem><Code2 />Visual Studio Code<Check className="fox-menu-check" /></DropdownMenuItem>
            <DropdownMenuItem><Terminal />Cursor</DropdownMenuItem>
            <DropdownMenuItem><ExternalLink />系统默认应用</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <IconButton label={terminalOpen ? '关闭终端' : '打开终端'} tooltipSide="bottom" active={terminalOpen} onClick={onTerminal}><Terminal size={16} /></IconButton>
        {state === 'running' && <Badge className="fox-running-badge"><span />运行中</Badge>}
        {state === 'question' && <Badge className="fox-question-badge"><CircleHelp size={12} />等待回答</Badge>}
        {state === 'approval' && <Badge className="fox-approval-badge"><ShieldCheck size={12} />等待批准</Badge>}
        {state === 'error' && <Badge className="fox-error-badge"><AlertTriangle size={12} />连接错误</Badge>}
        {state === 'denied' && <Badge className="fox-denied-badge"><CircleStop size={12} />已取消</Badge>}
        <DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多"><MoreHorizontal size={16} /></Button></DropdownMenuTrigger></TooltipTrigger>
            <TooltipContent side="bottom"><p>更多</p></TooltipContent>
          </Tooltip>
          <DropdownMenuContent align="end" className="fox-top-menu">
            <DropdownMenuLabel>当前对话</DropdownMenuLabel>
            <DropdownMenuItem onSelect={onFocusMode}><Maximize2 />{focusMode ? '退出专注模式' : '专注模式'}</DropdownMenuItem>
            <DropdownMenuItem onSelect={onDark}><SunMoon />{dark ? '切换到浅色' : '切换到深色'}</DropdownMenuItem>
            <DropdownMenuItem><UserRound />邀请协作</DropdownMenuItem>
            <DropdownMenuItem><Copy />复制链接</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem><Archive />归档对话</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </header>
  )
}

export function Workbench() {
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => readStoredBoolean(layoutStorage.sidebarCollapsed, false))
  const [sidebarWidth, setSidebarWidth] = useState(() => readStoredNumber(layoutStorage.sidebarWidth, 304, 280, 480))
  const [rightMode, setRightMode] = useState<RightMode | null>(readStoredRightMode)
  const [rightWidth, setRightWidth] = useState(() => readStoredNumber(layoutStorage.rightWidth, 360, 280, 760))
  const [dark, setDark] = useState(() => typeof window !== 'undefined' && window.localStorage.getItem(layoutStorage.theme) === 'dark')
  const [focusMode, setFocusMode] = useState(() => readStoredBoolean(layoutStorage.focusMode, false))
  const [mascotIndex, setMascotIndex] = useState(0)
  const [compactLayout, setCompactLayout] = useState(() => typeof window !== 'undefined' && window.matchMedia('(max-width: 1180px)').matches)
  const [compactRightOpen, setCompactRightOpen] = useState(false)
  const [emptyConversation, setEmptyConversation] = useState(false)
  const [terminalOpen, setTerminalOpen] = useState(false)
  const [terminalHeight, setTerminalHeight] = useState(() => readStoredNumber(layoutStorage.terminalHeight, 250, 180, 720))
  const [chatState, setChatState] = useState<ChatState>('complete')
  const [activePrompt, setActivePrompt] = useState('Kun 的协议是什么协议，我可以拿来二次开发并且企业内部使用吗？')
  const [composerResetKey, setComposerResetKey] = useState(0)
  const [suggestedPrompt, setSuggestedPrompt] = useState('')
  const workflowTimer = useRef<number | null>(null)

  useEffect(() => {
    document.documentElement.classList.toggle('dark', dark)
    window.localStorage.setItem(layoutStorage.theme, dark ? 'dark' : 'light')
  }, [dark])
  useEffect(() => { window.localStorage.setItem(layoutStorage.sidebarCollapsed, sidebarCollapsed ? '1' : '0') }, [sidebarCollapsed])
  useEffect(() => { window.localStorage.setItem(layoutStorage.sidebarWidth, String(Math.round(sidebarWidth))) }, [sidebarWidth])
  useEffect(() => { window.localStorage.setItem(layoutStorage.rightWidth, String(Math.round(rightWidth))) }, [rightWidth])
  useEffect(() => { window.localStorage.setItem(layoutStorage.rightMode, rightMode ?? 'closed') }, [rightMode])
  useEffect(() => { window.localStorage.setItem(layoutStorage.focusMode, focusMode ? '1' : '0') }, [focusMode])
  useEffect(() => { window.localStorage.setItem(layoutStorage.terminalHeight, String(Math.round(terminalHeight))) }, [terminalHeight])
  useEffect(() => {
    const interval = window.setInterval(() => setMascotIndex((value) => (value + 1) % sidebarMascots.length), 10000)
    return () => window.clearInterval(interval)
  }, [])
  useEffect(() => {
    const query = window.matchMedia('(max-width: 1180px)')
    const syncLayout = () => {
      setCompactLayout(query.matches)
      if (!query.matches) setCompactRightOpen(false)
    }
    syncLayout()
    query.addEventListener('change', syncLayout)
    return () => query.removeEventListener('change', syncLayout)
  }, [])
  useEffect(() => () => {
    if (workflowTimer.current !== null) window.clearTimeout(workflowTimer.current)
  }, [])

  const shellStyle = useMemo(() => ({ '--fox-sidebar-width': `${sidebarCollapsed ? 64 : sidebarWidth}px` } as React.CSSProperties), [sidebarCollapsed, sidebarWidth])

  const selectRightMode = (mode: RightMode) => {
    if (compactLayout) {
      if (compactRightOpen && rightMode === mode) {
        setCompactRightOpen(false)
        setRightMode(null)
        return
      }
      setRightMode(mode)
      setCompactRightOpen(true)
      return
    }
    setRightMode((current) => current === mode ? null : mode)
  }
  const toggleFocusMode = (value = !focusMode) => {
    setFocusMode(value)
    if (value) setCompactRightOpen(false)
  }
  const clearWorkflowTimer = () => {
    if (workflowTimer.current === null) return
    window.clearTimeout(workflowTimer.current)
    workflowTimer.current = null
  }
  const finishWorkflow = (delay = 2200) => {
    clearWorkflowTimer()
    setChatState('running')
    workflowTimer.current = window.setTimeout(() => {
      setChatState('complete')
      workflowTimer.current = null
    }, delay)
  }
  const resetConversation = (suggestion = '') => {
    clearWorkflowTimer()
    setChatState('complete')
    setEmptyConversation(!suggestion)
    setSuggestedPrompt(suggestion)
    setComposerResetKey((value) => value + 1)
  }

  return (
    <main className={`fox-shell ${focusMode ? 'is-focus-mode' : ''}`} style={shellStyle}>
      <WindowTitlebar />
      <div className="fox-workbench">
        <Sidebar collapsed={sidebarCollapsed || compactLayout} focusMode={focusMode} mascotIndex={mascotIndex} onCollapse={() => setSidebarCollapsed(!sidebarCollapsed)} onFocusMode={toggleFocusMode} onNewChat={() => resetConversation()} onOpenConversation={() => setEmptyConversation(false)} onTheme={() => setDark(!dark)} onSettings={() => toast('设置页将在下一阶段接入')} />
        {!sidebarCollapsed && !compactLayout && <SidebarResizeDivider onResize={(delta) => setSidebarWidth((value) => Math.min(480, Math.max(280, value + delta)))} />}
        <section className="fox-chat-pane">
          <ChatTopbar collapsed={sidebarCollapsed || compactLayout} onCollapse={() => setSidebarCollapsed(!sidebarCollapsed)} dark={dark} focusMode={focusMode} terminalOpen={terminalOpen} state={chatState} onDark={() => setDark(!dark)} onFocusMode={() => toggleFocusMode()} onTerminal={() => setTerminalOpen(!terminalOpen)} />
          <div className="fox-chat-stage"><Timeline empty={emptyConversation} state={chatState} prompt={activePrompt} onApprove={() => finishWorkflow()} onDeny={() => { clearWorkflowTimer(); setChatState('denied') }} onRetry={() => finishWorkflow()} onAnswer={(answer) => { toast.success(`已选择：${answer}`); finishWorkflow(2600) }} onStart={(suggestion) => resetConversation(suggestion)} /><Composer resetKey={composerResetKey} suggestedPrompt={suggestedPrompt} chatState={chatState} onSubmitPrompt={(prompt) => { clearWorkflowTimer(); setActivePrompt(prompt); setSuggestedPrompt(''); setEmptyConversation(false); if (/询问我|让我选择|需要确认方案|怎么处理/.test(prompt)) { setChatState('question'); return 'question' } if (/修改|写入|删除|重命名|创建文件/.test(prompt)) { setChatState('approval'); return 'approval' } if (/失败|错误|连接测试|检查连接/.test(prompt)) { setChatState('error'); return 'error' } setChatState('running'); return 'complete' }} onStatusChange={(status) => { if (status === 'ready' && chatState === 'running') setChatState('complete') }} /></div>
          {terminalOpen && <><TerminalResizeDivider onResize={(delta) => setTerminalHeight((value) => Math.min(720, Math.max(180, value + delta)))} /><AITerminal output={terminalOutput} isStreaming className="fox-terminal-drawer" style={{ height: terminalHeight }}><AITerminalHeader className="fox-terminal-head"><AITerminalTitle className="fox-terminal-title">powershell</AITerminalTitle><div className="fox-terminal-head-actions"><AITerminalStatus className="fox-terminal-status"><i />已连接</AITerminalStatus><AITerminalActions><AITerminalCopyButton className="fox-terminal-action" onCopy={() => toast.success('已复制终端输出')} /><IconButton label="关闭终端" onClick={() => setTerminalOpen(false)}><X size={14} /></IconButton></AITerminalActions></div></AITerminalHeader><AITerminalContent className="fox-terminal-content" /></AITerminal></>}
        </section>
        {rightMode && !compactLayout && <><ResizeDivider onResize={(delta) => setRightWidth((value) => Math.min(760, Math.max(280, value + delta)))} /><RightPanel mode={rightMode} width={rightWidth} onClose={() => setRightMode(null)} /></>}
        <SideRail mode={compactLayout && !compactRightOpen ? null : rightMode} onMode={selectRightMode} />
      </div>
      <Sheet open={compactLayout && compactRightOpen && Boolean(rightMode)} onOpenChange={(open) => { setCompactRightOpen(open); if (!open && compactLayout) setRightMode(null) }}>
        <SheetContent side="right" showCloseButton={false} className="fox-compact-context-sheet">
          {rightMode && <RightPanel compact mode={rightMode} width={rightWidth} onClose={() => { setCompactRightOpen(false); setRightMode(null) }} />}
        </SheetContent>
      </Sheet>
    </main>
  )
}
