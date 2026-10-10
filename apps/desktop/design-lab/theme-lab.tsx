import React from 'react'
import { createRoot } from 'react-dom/client'
import {
  ArrowUp,
  BookOpen,
  Bot,
  Brain,
  Check,
  ChevronDown,
  Copy,
  FileText,
  Folder,
  GitFork,
  Library,
  MessageSquare,
  MessageSquarePlus,
  Minus,
  MoreHorizontal,
  PanelRight,
  Paperclip,
  Puzzle,
  Search,
  Settings,
  ShieldCheck,
  Square,
  Terminal,
  ThumbsDown,
  ThumbsUp,
  UserRound,
  X,
} from 'lucide-react'
import './theme-lab.css'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Progress } from '@/components/ui/progress'
import { Separator } from '@/components/ui/separator'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { AppearanceThemeControls } from '@/features/settings/appearance-theme'

const THEMES = {
  graphite: 'A · 石墨',
  paper: 'B · 暖纸',
  mist: 'C · 雾蓝',
} as const
type ThemeId = keyof typeof THEMES

const params = new URLSearchParams(location.search)
const theme: ThemeId = (params.get('theme') as ThemeId) in THEMES ? (params.get('theme') as ThemeId) : 'graphite'
const dark = params.get('mode') === 'dark'
document.documentElement.dataset.colorTheme = theme
document.documentElement.classList.toggle('dark', dark)

const MASCOT = '/mascot/fox/idle/fox_sit_nicely.png'

function Titlebar() {
  return (
    <header className="flex h-9 shrink-0 items-center justify-between border-b border-sidebar-border bg-sidebar px-2 text-[13px]">
      <div className="flex items-center gap-2">
        <img src={MASCOT} alt="" className="size-5 rounded-[5px] object-contain" />
        <strong className="font-semibold">Fox</strong>
        <nav className="ml-2 flex items-center gap-0.5 text-muted-foreground">
          {['文件', '编辑', '视图', '帮助'].map((label) => (
            <span key={label} className="rounded-md px-2 py-1 hover:bg-sidebar-accent hover:text-foreground">{label}</span>
          ))}
        </nav>
      </div>
      <span className="text-xs text-muted-foreground">样稿 {THEMES[theme]} · {dark ? '深色' : '浅色'}</span>
      <div className="flex items-center text-muted-foreground">
        {[Minus, Square, X].map((Icon, index) => (
          <span key={index} className="grid h-9 w-11 place-items-center"><Icon className="size-3.5" /></span>
        ))}
      </div>
    </header>
  )
}

function SidebarRow({ icon: Icon, label, active = false, trailing }: { icon?: typeof Folder; label: string; active?: boolean; trailing?: React.ReactNode }) {
  return (
    <div className={`flex h-8 items-center gap-2 rounded-[var(--fox-radius-control)] px-2 text-[13.5px] ${active ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground' : 'text-sidebar-foreground/80 hover:bg-sidebar-accent/60'}`}>
      {Icon ? <Icon className="size-4 shrink-0 text-muted-foreground" /> : <span className="w-4 shrink-0" />}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {trailing}
    </div>
  )
}

function RunningDot() {
  return <span className="relative flex size-2"><span className="absolute inline-flex size-full animate-ping rounded-full bg-[var(--brand)] opacity-60" /><span className="relative inline-flex size-2 rounded-full bg-[var(--brand)]" /></span>
}

function Sidebar() {
  return (
    <aside className="flex w-[264px] shrink-0 flex-col border-r border-sidebar-border bg-sidebar px-2.5 py-3">
      <Button className="w-full justify-start" variant="outline"><MessageSquarePlus />新建对话</Button>
      <div className="relative mt-2.5">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input className="h-8 bg-background pl-8" placeholder="搜索对话与知识库" />
      </div>
      <nav className="mt-3 grid gap-0.5">
        <SidebarRow icon={MessageSquare} label="对话" active />
        <SidebarRow icon={Bot} label="数字同事" />
        <SidebarRow icon={Library} label="知识库" />
        <SidebarRow icon={Puzzle} label="插件中心" />
      </nav>
      <Separator className="my-3" />
      <div className="px-2 pb-1 text-xs font-medium text-muted-foreground">项目</div>
      <div className="grid gap-0.5">
        <SidebarRow icon={Folder} label="Fox 桌面端" trailing={<ChevronDown className="size-3.5 text-muted-foreground" />} />
        <div className="ml-4 grid gap-0.5 border-l border-sidebar-border pl-2">
          <SidebarRow label="重构设计变量" active />
          <SidebarRow label="assistant-ui 迁移调研" trailing={<RunningDot />} />
          <SidebarRow label="修复审批卡片的焦点问题" />
        </div>
        <SidebarRow icon={Folder} label="季度经营分析" />
      </div>
      <div className="px-2 pt-4 pb-1 text-xs font-medium text-muted-foreground">最近</div>
      <div className="grid gap-0.5">
        <SidebarRow label="整理 10 月客户访谈纪要" />
        <SidebarRow label="合同条款风险比对" trailing={<span className="text-[11px] text-muted-foreground">2 天前</span>} />
      </div>
      <div className="mt-auto flex items-center gap-2 rounded-[var(--fox-radius-control)] px-2 py-1.5">
        <Avatar size="sm"><AvatarFallback>胡</AvatarFallback></Avatar>
        <div className="min-w-0 flex-1 leading-tight">
          <div className="truncate text-[13px] font-medium">hury</div>
          <div className="truncate text-[11.5px] text-muted-foreground">本地工作区</div>
        </div>
        <Button variant="ghost" size="icon-sm" aria-label="设置"><Settings /></Button>
      </div>
    </aside>
  )
}

function ProcessStep({ icon: Icon, children, meta }: { icon: typeof Brain; children: React.ReactNode; meta?: string }) {
  return (
    <div className="flex items-center gap-2.5 py-1 text-[13px] text-muted-foreground">
      <Icon className="size-3.5 shrink-0" />
      <span className="min-w-0 flex-1 truncate text-foreground/80">{children}</span>
      {meta ? <span className="shrink-0 text-xs">{meta}</span> : null}
      <Check className="size-3.5 shrink-0 text-[var(--success)]" />
    </div>
  )
}

function Code({ children }: { children: React.ReactNode }) {
  return <code className="rounded-[var(--fox-radius-small)] bg-muted px-1.5 py-0.5 font-mono text-[12.5px]">{children}</code>
}

function Thread() {
  return (
    <div className="mx-auto flex w-full max-w-[760px] flex-col gap-6 px-6 pt-6 pb-4">
      <div className="flex items-center gap-3 text-xs text-muted-foreground"><Separator className="flex-1" />今天<Separator className="flex-1" /></div>

      <div className="flex flex-col items-end gap-1.5">
        <div className="max-w-[80%] rounded-[var(--fox-radius-large)] bg-[var(--user-bubble)] px-4 py-2.5 text-[14px] leading-relaxed">
          帮我梳理一下 globals.css 里的设计变量，哪些可以直接合并进 shadcn 的语义变量？
        </div>
        <div className="flex items-center gap-1 text-muted-foreground">
          <Button variant="ghost" size="icon-xs" aria-label="复制"><Copy /></Button>
          <Button variant="ghost" size="icon-xs" aria-label="分叉"><GitFork /></Button>
        </div>
      </div>

      <div className="flex gap-3">
        <Avatar size="sm" className="mt-0.5 bg-muted"><AvatarImage src={MASCOT} /><AvatarFallback>F</AvatarFallback></Avatar>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 text-[13px]">
            <span className="font-medium">Fox 默认助手</span>
            <span className="text-muted-foreground">· 用时 18 秒</span>
          </div>

          <div className="mt-2.5 rounded-[var(--fox-radius-card)] border border-border bg-card px-3.5 py-2">
            <div className="flex items-center gap-2 pb-1 text-[13px] font-medium">
              <Brain className="size-4 text-muted-foreground" />已完成分析
              <span className="font-normal text-muted-foreground">· 4 个步骤</span>
              <ChevronDown className="ml-auto size-4 text-muted-foreground" />
            </div>
            <ProcessStep icon={FileText} meta="369 行">读取 src/styles/globals.css</ProcessStep>
            <ProcessStep icon={Search} meta="3 条结果">检索知识库「设计规范」</ProcessStep>
            <ProcessStep icon={Terminal} meta="0.4 秒">运行 rg -o "var(--fox-[a-z-]+" --count</ProcessStep>
          </div>

          <div className="mt-4 space-y-3 text-[14px] leading-[1.75]">
            <p>共有 115 个 <Code>--fox-*</Code> 变量在使用，可以分三类处理：</p>
            <ul className="list-disc space-y-1.5 pl-5 marker:text-muted-foreground">
              <li><strong className="font-semibold">直接合并</strong>：<Code>--fox-text</Code>、<Code>--fox-muted</Code>、<Code>--fox-border</Code> 与 shadcn 变量一一对应，可改为别名后逐步删除。</li>
              <li><strong className="font-semibold">扩展为语义变量</strong>：成功、警告、品牌色进入 <Code>@theme</Code>，供 Tailwind 直接使用。</li>
              <li><strong className="font-semibold">交给 Tailwind 刻度</strong>：间距和字号改用标准类名。</li>
            </ul>
            <pre className="overflow-hidden rounded-[var(--fox-radius-card)] border border-border bg-muted/60 px-4 py-3 font-mono text-[12.5px] leading-relaxed text-foreground/90">{`--fox-text: var(--foreground);   /* 别名，下一阶段删除 */`}</pre>
          </div>

          <Card size="sm" className="mt-4 border-[color-mix(in_oklch,var(--warning)_35%,var(--border))]">
            <CardHeader>
              <CardTitle className="flex items-center gap-2"><ShieldCheck className="size-4 text-[var(--warning)]" />需要你的确认</CardTitle>
              <CardDescription>Fox 想要写入 2 个文件：src/styles/globals.css、components.json</CardDescription>
            </CardHeader>
            <CardContent className="flex justify-end gap-2">
              <Button variant="outline" size="sm">拒绝</Button>
              <Button variant="outline" size="sm">总是允许</Button>
              <Button size="sm">允许一次</Button>
            </CardContent>
          </Card>

          <div className="mt-3 flex items-center gap-1 text-muted-foreground">
            <Button variant="ghost" size="icon-xs" aria-label="复制"><Copy /></Button>
            <Button variant="ghost" size="icon-xs" aria-label="分叉"><GitFork /></Button>
            <Button variant="ghost" size="icon-xs" aria-label="有帮助"><ThumbsUp /></Button>
            <Button variant="ghost" size="icon-xs" aria-label="没帮助"><ThumbsDown /></Button>
            <span className="ml-1 text-xs">DeepSeek V4 · 14:42</span>
          </div>
        </div>
      </div>
    </div>
  )
}

function Composer() {
  return (
    <div className="mx-auto w-full max-w-[760px] px-6 pb-5">
      <div className="rounded-[var(--fox-radius-large)] border border-input bg-card shadow-[0_1px_2px_oklch(0_0_0/4%),0_8px_24px_-12px_oklch(0_0_0/12%)]">
        <div className="min-h-[56px] px-4 pt-3.5 text-[14px] text-muted-foreground">给 Fox 发送消息，@ 引用文件，/ 使用技能</div>
        <div className="flex items-center gap-1.5 px-2.5 pb-2.5">
          <Button variant="ghost" size="icon-sm" aria-label="添加附件"><Paperclip /></Button>
          <Badge variant="outline" className="h-6 gap-1 px-2 font-normal"><BookOpen />设计规范</Badge>
          <Badge variant="outline" className="h-6 gap-1 px-2 font-normal"><UserRound />前端架构师</Badge>
          <span className="flex-1" />
          <Button variant="ghost" size="sm" className="text-muted-foreground">DeepSeek V4<ChevronDown /></Button>
          <Button size="icon-sm" className="rounded-full" aria-label="发送"><ArrowUp /></Button>
        </div>
      </div>
    </div>
  )
}

function ArtifactRow({ name, meta, verified = false }: { name: string; meta: string; verified?: boolean }) {
  return (
    <div className="flex items-center gap-3 rounded-[var(--fox-radius-control)] border border-border bg-card px-3 py-2.5">
      <div className="grid size-8 shrink-0 place-items-center rounded-[var(--fox-radius-small)] bg-muted text-muted-foreground"><FileText className="size-4" /></div>
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px] font-medium">{name}</div>
        <div className="text-xs text-muted-foreground">{meta}</div>
      </div>
      {verified ? <Badge variant="outline" className="border-[color-mix(in_oklch,var(--success)_35%,var(--border))] text-[var(--success)]"><Check />已验证</Badge> : null}
    </div>
  )
}

function RightPanel() {
  return (
    <aside className="flex w-[320px] shrink-0 flex-col gap-4 border-l border-border bg-sidebar px-4 py-3">
      <Tabs defaultValue="artifacts">
        <TabsList className="w-full">
          <TabsTrigger value="files">文件</TabsTrigger>
          <TabsTrigger value="artifacts">产物</TabsTrigger>
          <TabsTrigger value="activity">活动</TabsTrigger>
        </TabsList>
      </Tabs>
      <div className="grid gap-2">
        <ArtifactRow name="变量合并表.md" meta="12 KB · 刚刚" verified />
        <ArtifactRow name="theme-graphite.css" meta="3 KB · 2 分钟前" />
        <ArtifactRow name="迁移检查清单.xlsx" meta="28 KB · 5 分钟前" verified />
      </div>
      <Card size="sm">
        <CardHeader>
          <CardTitle>运行概况</CardTitle>
          <CardDescription>本轮对话的模型与资源使用</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3 text-[13px]">
          <div className="flex justify-between"><span className="text-muted-foreground">模型</span><span>DeepSeek V4</span></div>
          <div className="flex justify-between"><span className="text-muted-foreground">工具调用</span><span>7 次</span></div>
          <div className="grid gap-1.5">
            <div className="flex justify-between"><span className="text-muted-foreground">上下文</span><span>42%</span></div>
            <Progress value={42} />
          </div>
          <Separator />
          <div className="flex items-center justify-between gap-3"><span>自动批准只读操作</span><Switch defaultChecked /></div>
          <div className="flex items-center justify-between gap-3"><span>完成后发送通知</span><Switch /></div>
        </CardContent>
      </Card>
    </aside>
  )
}

function Workbench() {
  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <Titlebar />
      <div className="flex min-h-0 flex-1">
        <Sidebar />
        <main className="flex min-w-0 flex-1 flex-col">
          <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-5">
            <span className="text-[14px] font-medium">重构设计变量</span>
            <Badge variant="secondary">Fox 桌面端</Badge>
            <span className="flex-1" />
            <Button variant="ghost" size="icon-sm" aria-label="更多"><MoreHorizontal /></Button>
            <Button variant="ghost" size="icon-sm" aria-label="右侧面板"><PanelRight /></Button>
          </div>
          <div className="min-h-0 flex-1 overflow-hidden"><Thread /></div>
          <Composer />
        </main>
        <RightPanel />
      </div>
    </div>
  )
}

function AppearancePreview() {
  const [isDark, setIsDark] = React.useState(dark)
  const [colorTheme, setColorTheme] = React.useState<ThemeId>(theme)
  React.useEffect(() => {
    document.documentElement.classList.toggle('dark', isDark)
    document.documentElement.dataset.colorTheme = colorTheme
  }, [isDark, colorTheme])
  return (
    <div className="h-screen overflow-auto bg-background p-10 text-foreground">
      <Card className="mx-auto max-w-[720px] px-5">
        <h2 className="text-[length:var(--fox-type-section)] font-semibold">外观</h2>
        <p className="mb-2 text-[length:var(--fox-type-meta)] text-muted-foreground">保持 Fox 的主题、字体和界面密度一致。</p>
        <AppearanceThemeControls dark={isDark} onDarkChange={setIsDark} colorTheme={colorTheme} onColorThemeChange={setColorTheme} />
      </Card>
    </div>
  )
}

createRoot(document.getElementById('root')!).render(params.get('view') === 'appearance' ? <AppearancePreview /> : <Workbench />)
