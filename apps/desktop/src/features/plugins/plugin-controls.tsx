import { Blocks, Cable, Database, File, FileText, Folder, Globe2, NotebookPen, Puzzle, Search, ShieldCheck, Terminal, Wrench, type LucideIcon } from 'lucide-react'
import directoryMarks from './directory-marks.svg?url'
import directoryMarkIds from './directory-mark-ids.json'
import { brandImages } from './brand-images'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { LifecycleHooksSection } from '@/features/settings/lifecycle-hooks-section'
import type { NavigateWorkspace } from '@/features/workspace/types'
import type { PluginCardView } from './model'
import { isPluginSection, pluginSection, pluginSectionLabels, pluginSections, pluginStatus, pluginStatusLabel, type PluginSection } from './plugin-browser-data'
import '@/styles/plugin-center.css'

const icons = { mcp: Cable, plugin: Globe2, skill: Puzzle, tool: Wrench }
const catalogIcons: Record<string, LucideIcon> = { cable: Cable, database: Database, file: File, 'file-text': FileText, folder: Folder, globe: Globe2, notebook: NotebookPen, puzzle: Puzzle, search: Search, terminal: Terminal, wrench: Wrench }

const standaloneMarks: Record<string, string> = { 'skill:fox-office-word': 'office-word', 'skill:fox-office-excel': 'office-excel', 'skill:fox-office-ppt': 'office-powerpoint', 'search:brave': 'brave', 'search:tavily': 'tavily' }
const markAliases: Record<string, string> = {
  'feishu-bot': 'feishu',
  'wecom-bot': 'wecom',
  'outlook-calendar': 'outlook',
  'minimax-plan': 'minimax-media',
  'zai-reader': 'zai-search',
}
const spriteMarks = new Set(directoryMarkIds)

export function PluginMark({ kind, name, id }: { kind: PluginSection | 'tool'; name?: string; id?: string }) {
  const prefix = 'mcp:fox-catalog-'
  const mark = id?.startsWith(prefix) ? id.slice(prefix.length) : id && standaloneMarks[id]
  const brand = mark && (markAliases[mark] ?? mark)
  const image = brand && brandImages[brand]
  if (image) return <span className="fox-extension-mark is-directory-mark" aria-hidden="true"><img src={image} alt="" /></span>
  if (brand && spriteMarks.has(brand)) return <span className="fox-extension-mark is-directory-mark" aria-hidden="true"><svg><use href={directoryMarks + '#' + brand} /></svg></span>
  const Icon = (name && catalogIcons[name]) || icons[kind]
  return <span className="fox-extension-mark" aria-hidden="true"><Icon /></span>
}

export function PluginStatusBadge({ card, label }: { card: PluginCardView; label?: string }) {
  return <Badge variant="outline" className={`fox-extension-status is-${pluginStatus(card)}`}>{label ?? pluginStatusLabel(card)}</Badge>
}

export function PluginTabs({ section, counts, onChange }: {
  section: PluginSection; counts?: Record<PluginSection, number>; onChange: (section: PluginSection) => void
}) {
  return <Tabs value={section} onValueChange={value => { if (isPluginSection(value)) onChange(value) }}>
    <TabsList aria-label="插件类型">{pluginSections.map(kind => {
      const Icon = kind === 'plugin' ? Blocks : icons[kind]
      return <TabsTrigger key={kind} value={kind}><Icon aria-hidden="true" size={14} />
        {pluginSectionLabels[kind]}{counts && <span className="fox-extension-count">{counts[kind]}</span>}
      </TabsTrigger>
    })}</TabsList>
  </Tabs>
}

export function PluginPolicySheet({ open, onOpenChange, navigate }: { open: boolean; onOpenChange: (open: boolean) => void; navigate?: NavigateWorkspace }) {
  return <Sheet open={open} onOpenChange={onOpenChange}>
    <SheetContent className="fox-extension-policy-sheet" side="right">
      <SheetHeader><SheetTitle>插件策略与 Hook</SheetTitle><SheetDescription>管理工具调用和运行过程中的规则。</SheetDescription></SheetHeader>
      <div className="fox-extension-policy-body">
        {navigate && <Button variant="outline" onClick={() => navigate('settings-security')}><ShieldCheck />权限与安全</Button>}
        <LifecycleHooksSection />
      </div>
    </SheetContent>
  </Sheet>
}

export function SkillImportDialog({ open, onOpenChange, onRescan }: {
  open: boolean; onOpenChange: (open: boolean) => void; onRescan: () => void
}) {
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent>
      <DialogHeader><DialogTitle>导入技能</DialogTitle><DialogDescription>将包含 SKILL.md 的技能文件夹放入 Fox 数据目录的 skills 子目录，然后重新扫描。</DialogDescription></DialogHeader>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>取消</Button>
        <Button onClick={() => { onRescan(); onOpenChange(false) }}>重新扫描</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

export function openPluginDetail(navigate: NavigateWorkspace, card: PluginCardView, fromSettings = false) {
  const section = pluginSection(card)
  if (section !== 'tool') navigate(fromSettings ? 'settings-plugin-detail' : 'plugin-detail', card.id, { documentId: section })
}
