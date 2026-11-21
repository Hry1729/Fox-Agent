import { useState } from 'react'
import { ChevronRight, FolderOpen, Users } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { ContactSettings } from '@/features/companions/profile'
import { useContacts } from '@/features/companions/context'
import {
  COMPANION_DEFAULT_MODEL_KEY,
  COMPANION_DEFAULT_PERMISSION_KEY,
  COMPANION_DEFAULT_REMINDER_KEY,
  readCompanionDefaultModel,
  readCompanionDefaultPermission,
  readCompanionDefaultReminder,
  type CompanionPermission,
} from '@/features/companions/defaults'
import type { ReminderMode } from '@/features/companions/api'
import { notify as toast } from '@/features/notifications'
import { SettingsGroup, SettingsPageLayout, SettingsRow } from './settings-layout'
import { useModelProviders } from './use-model-providers'

function companionRoot(workspaceDir: string, id: string) {
  const index = workspaceDir.lastIndexOf(id)
  return index > 0 ? workspaceDir.slice(0, index).replace(/[\\/]$/, '') : null
}

export function CompanionsSettingsPage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const providers = useModelProviders()
  const contacts = useContacts()
  const [permission, setPermission] = useState<CompanionPermission>(readCompanionDefaultPermission)
  const [model, setModel] = useState(readCompanionDefaultModel)
  const [reminder, setReminder] = useState<ReminderMode>(readCompanionDefaultReminder)
  const [saved, setSaved] = useState({ permission, model, reminder })
  const dirty = saved.permission !== permission || saved.model !== model || saved.reminder !== reminder
  const sample = contacts?.list.companions.find(item => !item.deletingAt)
  const root = sample ? companionRoot(sample.workspaceDir, sample.id)?.replace(/\\/g, '/') : null

  const save = () => {
    window.localStorage.setItem(COMPANION_DEFAULT_PERMISSION_KEY, permission)
    window.localStorage.setItem(COMPANION_DEFAULT_MODEL_KEY, model)
    window.localStorage.setItem(COMPANION_DEFAULT_REMINDER_KEY, reminder)
    setSaved({ permission, model, reminder })
    toast.success('伙伴默认值已保存')
  }

  return <WorkspacePage title="伙伴" subtitle="工作目录与新伙伴默认值" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout title="伙伴" description="设置新伙伴的工作默认值。已有伙伴的模型、权限和文件位置保持不变。" actions={<Button variant="outline" onClick={() => navigate('companions')}><Users />管理已有伙伴<ChevronRight /></Button>}>
      <form className="fox-companion-defaults" onSubmit={event => { event.preventDefault(); if (dirty) save() }}>
        <SettingsGroup title="工作目录">
          <div className="fox-companion-directory">
            <div className="fox-settings-row-text">
              <b>伙伴工作目录</b>
              <small>每位伙伴有独立目录，工作文件和笔记分开保存。</small>
            </div>
            <div className="fox-companion-directory-path">
              <FolderOpen aria-hidden="true" />
              <code>{root ?? 'Fox 数据目录/companions'}</code>
            </div>
            <ul className="fox-companion-directory-folders">
              <li><code>workspace/</code> 工作文件</li>
              <li><code>notes/</code> 伙伴笔记</li>
            </ul>
          </div>
          <p className="fox-settings-footnote">目录由 Fox 数据目录决定。这里不会搬迁已有伙伴；已有伙伴的目录可在其资料页打开或修复。</p>
        </SettingsGroup>

        <SettingsGroup title="新伙伴默认值" aside="仅影响之后创建的伙伴">
          <SettingsRow title="默认执行方式" description="私聊中写入文件或运行命令前的确认方式">
            <select aria-label="新伙伴默认执行方式" value={permission} onChange={event => setPermission(event.target.value as CompanionPermission)}>
              <option value="read_only">只读</option><option value="ask">询问</option><option value="allow">自动</option>
            </select>
          </SettingsRow>
          <SettingsRow title="默认模型" description="沿用全局会使用模型页设置的默认模型">
            <select aria-label="新伙伴默认模型" value={model} disabled={providers.busyId !== null} onChange={event => setModel(event.target.value)}>
              <option value="">沿用全局默认模型</option>
              {providers.items.flatMap(provider => provider.models.map(item => <option key={`${provider.id}:${item.id}`} value={`${provider.id}::${item.modelId}`}>{provider.name} · {item.displayName || item.modelId}</option>))}
            </select>
          </SettingsRow>
          <SettingsRow title="默认提醒方式" description="沿用全局会使用通知设置；静音仍保留未读与待处理标记">
            <select aria-label="新伙伴默认提醒方式" value={reminder} onChange={event => setReminder(event.target.value as ReminderMode)}>
              <option value="follow_global">沿用全局</option><option value="all">全部提醒</option><option value="action_only">仅需要处理</option><option value="muted">静音</option>
            </select>
          </SettingsRow>
          {providers.error && <p className="fox-settings-error" role="alert">模型列表暂时不可用：{providers.error}。其他默认值仍可保存。</p>}
          <div className="fox-companion-defaults-actions"><span>{dirty ? '有尚未保存的默认值' : '默认值已保存'}</span><Button type="submit" disabled={!dirty}>保存伙伴默认值</Button></div>
        </SettingsGroup>
      </form>

      <SettingsGroup title="归档与保留目录">
        <ContactSettings />
      </SettingsGroup>
    </SettingsPageLayout>
  </WorkspacePage>
}
