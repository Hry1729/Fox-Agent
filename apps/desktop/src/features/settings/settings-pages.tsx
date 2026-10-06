import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { Activity, Archive, BellRing, Bot, Cable, Check, ChevronDown, ChevronRight, Copy, Download, ExternalLink, FileText, FileWarning, FolderOpen, HardDrive, Info, Keyboard, LoaderCircle, LogIn, MessageSquare, Minus, Moon, MoreHorizontal, Pencil, Plus, Puzzle, RotateCcw, Server, Shield, ShieldCheck, Sparkles, Sun, Trash2, Volume2, Wrench, Zap } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { AnimatePresence, motion } from 'motion/react'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Separator } from '@/components/ui/separator'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { useYuxiService } from './use-yuxi-service'
import { useModelService } from './use-model-service'
import { useModelProviders, type SaveModelProviderInput } from './use-model-providers'
import { useYuxiUser } from './use-yuxi-user'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { useSkills } from './use-skills'
import { useMcpServers, type SaveMcpServerInput } from './use-mcp-servers'
import { useLifecycleHooks } from './use-lifecycle-hooks'
import type { AgentRecord, KnowledgePreviewCacheStatistics, LifecycleHookRecord, McpServerRecord, ModelProviderRecord, NotificationPreferencesRecord, ObservabilityStatistics, ProjectManagementRecord, ProjectRecord, UsageStatistics } from '@/features/conversations/model/types'
import { Grainient } from '@/components/effects/grainient'
import { BorderBeam } from '@/components/effects/border-beam'
import { UserProfileDialog, useUserProfile } from '@/features/profile/user-profile'
import { resolveYuxiLoginReturn, resolveYuxiServiceReturn, YUXI_LOGIN_RETURN_KEY, YUXI_SERVICE_RETURN_KEY } from '@/features/knowledge/knowledge-navigation'
import { normalizeTextScale, persistTextScale, readTextScale, TEXT_SCALE_MAX, TEXT_SCALE_MIN } from './text-scale'
import { persistProcessDisplayMode, useProcessDisplayMode, type ProcessDisplayMode } from '@/features/chat/process-display-mode'
import { defaultLocalKnowledgeGateway } from '@/features/local-knowledge/tauri-gateway'
import { defaultPluginGateway } from '@/features/plugins/tauri-gateway'
import {
  DEFAULT_NOTIFICATION_SOUND,
  NOTIFICATION_SOUND_OPTIONS,
  dispatchNotificationPreferencesChanged,
  dispatchNotificationPreview,
  normalizeNotificationSoundId,
  playNotificationSound,
} from '@/features/notifications'

function serviceStatusLabel(status?: string) {
  if (status === 'connected') return '已连接'
  if (status === 'unavailable') return '不可用'
  return '未检测'
}

function knowledgeServiceName(name?: string | null) {
  const value = name?.trim()
  if (!value || /^yuxi(?:\s*(?:service|服务))?$/i.test(value)) return '知识库服务'
  return value
}

function connectionTypeLabel(type?: string) {
  if (type === 'local') return '本地服务'
  if (type === 'lan') return '局域网服务'
  return '远程服务'
}

function useStoredPreference<T extends string | boolean>(key: string, fallback: T) {
  const [value, setValue] = useState<T>(() => {
    if (typeof window === 'undefined') return fallback
    const stored = window.localStorage.getItem(key)
    if (stored === null) return fallback
    if (typeof fallback === 'boolean') return (stored === '1') as T
    return stored as T
  })
  const update = (next: T) => {
    setValue(next)
    window.localStorage.setItem(key, typeof next === 'boolean' ? next ? '1' : '0' : next)
  }
  return [value, update] as const
}

function SettingsScaffold({ title, kicker, description, children }: { title: string; kicker: string; description: string; children: ReactNode }) {
  void kicker
  return <div className="fox-settings-layout"><main><header><h1>{title}</h1><p>{description}</p></header>{children}</main></div>
}

function PreferenceRow({ title, description, children }: { title: string; description: string; children: ReactNode }) {
  return <div className="fox-setting-row fox-setting-row-bordered"><span><b>{title}</b><small>{description}</small></span>{children}</div>
}

const DEFAULT_NOTIFICATION_PREFERENCES: NotificationPreferencesRecord = {
  systemPopup: false,
  sound: false,
  soundId: DEFAULT_NOTIFICATION_SOUND,
  badge: true,
  quietProgress: true,
  updatedAt: 0,
}

function NotificationSettingsSection() {
  const [preferences, setPreferences] = useState<NotificationPreferencesRecord | null>(null)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    let cancelled = false
    if (!desktopRuntimeAvailable) {
      setPreferences(DEFAULT_NOTIFICATION_PREFERENCES)
      return
    }
    void desktopClient.getNotificationPreferences().then((value) => {
      if (!cancelled) setPreferences({ ...value, soundId: normalizeNotificationSoundId(value.soundId) })
    }).catch((cause) => {
      if (!cancelled) {
        setPreferences(DEFAULT_NOTIFICATION_PREFERENCES)
        toast.error('通知设置读取失败', { description: desktopErrorDetails(cause).message })
      }
    })
    return () => { cancelled = true }
  }, [])

  const updatePreferences = async (changes: Partial<NotificationPreferencesRecord>) => {
    if (!preferences || saving) return
    let nextChanges = changes
    if (changes.systemPopup === true && typeof Notification !== 'undefined' && Notification.permission === 'default') {
      const permission = await Notification.requestPermission()
      if (permission !== 'granted') {
        toast.error('系统通知权限未开启')
        nextChanges = { ...changes, systemPopup: false }
      }
    }
    const next = { ...preferences, ...nextChanges }
    setPreferences(next)
    if (!desktopRuntimeAvailable) {
      dispatchNotificationPreferencesChanged(next)
      return
    }
    setSaving(true)
    try {
      const saved = await desktopClient.saveNotificationPreferences(next)
      const normalized = { ...saved, soundId: normalizeNotificationSoundId(saved.soundId) }
      setPreferences(normalized)
      dispatchNotificationPreferencesChanged(normalized)
    } catch (cause) {
      setPreferences(preferences)
      toast.error('通知设置保存失败', { description: desktopErrorDetails(cause).message })
    } finally {
      setSaving(false)
    }
  }

  if (!preferences) {
    return <Card className="fox-settings-section fox-notification-settings"><div className="fox-notification-settings-loading"><LoaderCircle className="animate-spin" />正在读取通知设置</div></Card>
  }

  const soundId = normalizeNotificationSoundId(preferences.soundId)
  return <Card className="fox-settings-section fox-notification-settings">
    <div className="fox-settings-section-head"><div><h2>通知</h2><p>管理通知展示、提示音与普通进度的打扰程度。</p></div><Button variant="outline" size="sm" onClick={() => dispatchNotificationPreview(soundId)}><BellRing />测试通知</Button></div>
    <PreferenceRow title="系统弹窗" description="允许 Fox 同时发送操作系统通知"><Switch checked={preferences.systemPopup} disabled={saving} onCheckedChange={(value) => void updatePreferences({ systemPopup: value })} /></PreferenceRow>
    <PreferenceRow title="通知声音" description="为需要关注的通知播放提示音"><div className="fox-notification-sound-control"><Switch checked={preferences.sound} disabled={saving} onCheckedChange={(value) => void updatePreferences({ sound: value })} /><Select value={soundId} disabled={saving || !preferences.sound} onValueChange={(value) => { const next = normalizeNotificationSoundId(value); playNotificationSound(next); void updatePreferences({ soundId: next }) }}><SelectTrigger className="fox-settings-select"><Volume2 /><SelectValue /></SelectTrigger><SelectContent>{NOTIFICATION_SOUND_OPTIONS.map((option) => <SelectItem key={option.id} value={option.id} title={option.description}>{option.label}</SelectItem>)}</SelectContent></Select></div></PreferenceRow>
    <PreferenceRow title="未读提示点" description="有未读通知时在右上角显示红点"><Switch checked={preferences.badge} disabled={saving} onCheckedChange={(value) => void updatePreferences({ badge: value })} /></PreferenceRow>
    <PreferenceRow title="静默普通进度" description="合并普通进度通知，审批、提问和失败仍会提醒"><Switch checked={preferences.quietProgress} disabled={saving} onCheckedChange={(value) => void updatePreferences({ quietProgress: value })} /></PreferenceRow>
  </Card>
}

export function SettingsPage({ sidebarCollapsed, onSidebar, navigate, dark, onDark }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; dark: boolean; onDark: () => void }) {
  const yuxi = useYuxiService()
  const yuxiUser = useYuxiUser(Boolean(yuxi.service?.credentialConfigured))
  const [textScale, setTextScale] = useState(() => readTextScale())
  const processDisplayMode = useProcessDisplayMode()
  const [profileDialogOpen, setProfileDialogOpen] = useState(false)
  const { profile, save: saveProfile } = useUserProfile({ name: yuxiUser.user?.username, avatar: yuxiUser.user?.avatar })
  const profileName = profile.name
  const profileDetail = yuxiUser.user?.departmentName ?? (yuxiUser.user ? `知识库账户 · ${yuxiUser.user.role}` : '本地用户')
  const profileFallback = profile.initial
  useEffect(() => {
    persistTextScale(textScale)
  }, [textScale])

  const changeTextScale = (next: number) => {
    setTextScale(normalizeTextScale(next))
  }

  return (
    <WorkspacePage title="设置" subtitle="应用与外观" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
      <SettingsScaffold kicker="桌面偏好" title="应用" description="调整 Fox 在这台设备上的显示和界面密度。">
          <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>个人资料</h2><p>设置 Fox 在这台设备上显示的头像与用户名。</p></div><div className="fox-profile-setting-actions"><Button variant="outline" size="sm" onClick={() => setProfileDialogOpen(true)}><Pencil />编辑资料</Button>{!yuxiUser.user && <Button variant="outline" size="sm" onClick={() => navigate('login')}>知识库登录</Button>}</div></div><div className="fox-profile-setting"><Avatar><AvatarImage src={profile.avatar} alt="" /><AvatarFallback>{profileFallback}</AvatarFallback></Avatar><span><b>{profileName}</b><small>{yuxiUser.user?.uid ?? 'fox-local'}</small><em>{profileDetail}</em></span></div></Card>
          <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>外观</h2><p>保持 Fox 的主题、字体和界面密度一致。</p></div></div><div className="fox-theme-grid"><button className={!dark ? 'is-active' : ''} onClick={() => dark && onDark()}><span className="fox-theme-preview is-light"><i /><b /></span><em><Sun />浅色{!dark && <Check />}</em></button><button className={dark ? 'is-active' : ''} onClick={() => !dark && onDark()}><span className="fox-theme-preview is-dark"><i /><b /></span><em><Moon />深色{dark && <Check />}</em></button></div><PreferenceRow title="文字大小" description="调整 Fox 各界面的文字显示比例"><div className="fox-text-scale-control"><span className="fox-text-scale-a is-small" aria-hidden="true">A</span><input type="range" min={TEXT_SCALE_MIN} max={TEXT_SCALE_MAX} step={1} value={textScale} aria-label="文字大小" onChange={(event) => changeTextScale(Number(event.target.value))} /><span className="fox-text-scale-a is-large" aria-hidden="true">A</span><div className="fox-text-scale-stepper"><Button type="button" variant="ghost" size="icon" aria-label="缩小文字" onClick={() => changeTextScale(textScale - 5)}><Minus /></Button><label><input type="number" min={TEXT_SCALE_MIN} max={TEXT_SCALE_MAX} step={1} value={textScale} aria-label="文字大小百分比" onChange={(event) => changeTextScale(Number(event.target.value))} /><span>%</span></label><Button type="button" variant="ghost" size="icon" aria-label="放大文字" onClick={() => changeTextScale(textScale + 5)}><Plus /></Button></div></div></PreferenceRow><PreferenceRow title="工作步骤展示" description="选择希望看到多少工具调用细节"><Select value={processDisplayMode} onValueChange={(value) => persistProcessDisplayMode(value as ProcessDisplayMode)}><SelectTrigger className="fox-settings-select" aria-label="工作步骤展示"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="compact">简洁</SelectItem><SelectItem value="standard">标准</SelectItem><SelectItem value="detailed">详细</SelectItem><SelectItem value="verbose">完全展开</SelectItem></SelectContent></Select></PreferenceRow></Card>
          <NotificationSettingsSection />
          <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>首次设置向导</h2></div></div><PreferenceRow title="重新运行设置向导" description="检查模型服务、知识库连接和 Fox 的基础使用方式"><Button variant="outline" size="sm" onClick={() => navigate('onboarding')}><Sparkles />打开向导</Button></PreferenceRow></Card>
        <UserProfileDialog open={profileDialogOpen} onOpenChange={setProfileDialogOpen} profile={profile} detail={profileDetail} onSave={async (next) => { await saveProfile(next); toast.success('个人资料已更新') }} />
      </SettingsScaffold>
    </WorkspacePage>
  )
}

export function OnboardingPage({ navigate, onExit, dark = false }: { navigate: NavigateWorkspace; onExit: (status: 'completed' | 'skipped') => void; dark?: boolean }) {
  const model = useModelService()
  const yuxi = useYuxiService()
  const [step, setStep] = useState(0)
  const [stepDirection, setStepDirection] = useState(1)
  const steps = [
    { title: '连接模型服务', description: '配置 Fox 原生 Agent 使用的模型、API 协议与上下文能力。', ready: Boolean(model.service), action: () => navigate('settings-models'), actionLabel: model.service ? '检查模型配置' : '配置模型服务' },
    { title: '连接知识库服务', description: '连接本机或局域网服务，启用远程专家、知识库和知识图谱。', ready: Boolean(yuxi.service), action: () => navigate('settings-yuxi'), actionLabel: yuxi.service ? '检查知识库连接' : '配置知识库服务' },
    { title: '开始使用 Fox', description: '模型服务是原生专家的必要条件；知识库服务可以稍后连接。', ready: Boolean(model.service), action: () => onExit('completed'), actionLabel: '进入 Fox' },
  ]
  const current = steps[step]
  const statusCopy = step === 0
    ? model.service ? `${model.service.name} · ${model.service.modelId}` : '尚未配置模型服务，原生专家无法发送消息'
    : step === 1
      ? yuxi.service ? `${knowledgeServiceName(yuxi.service.name)} · ${yuxi.service.baseUrl}` : '尚未配置知识库服务，知识库与远程专家不可用'
      : model.service ? '基础配置已完成，可以开始创建对话' : '你可以跳过，但需要配置模型服务后才能正常对话'
  const advance = () => {
    setStepDirection(1)
    setStep((value) => Math.min(steps.length - 1, value + 1))
  }
  const goBack = () => {
    setStepDirection(-1)
    setStep((value) => Math.max(0, value - 1))
  }
  // The final step is a closing screen rather than another wizard page: the left
  // column collapses, the fox artwork and its two lines slide into the centre of the
  // full-width card, and the "进入 Fox" call to action appears underneath.
  const finishing = step === steps.length - 1
  return <div className={`fox-onboarding-screen${finishing ? ' is-finishing' : ''}`} data-tauri-drag-region>
    <Grainient
      className="fox-onboarding-background"
      color1={dark ? '#173a63' : '#9fc9ee'}
      color2={dark ? '#0a1a2c' : '#e8f4ff'}
      color3={dark ? '#03060b' : '#ffffff'}
      timeSpeed={0.6}
      colorBalance={-0.18}
      warpStrength={1}
      warpFrequency={5}
      warpSpeed={2}
      warpAmplitude={50}
      blendAngle={0}
      blendSoftness={0.05}
      rotationAmount={500}
      noiseScale={2}
      grainAmount={0.1}
      grainScale={2}
      contrast={1.12}
      saturation={1.05}
      zoom={0.9}
      dpr={1}
    />
    <Button variant="outline" size="sm" className="fox-onboarding-skip" onClick={() => onExit('skipped')}>跳过设置</Button>
    <div className="fox-onboarding-shell">
      <section className="fox-onboarding-main">
        <header className="fox-onboarding-brand"><span><img src="/mascot/fox/idle/fox_sit_nicely.png" alt="" />Fox</span></header>
        <main className="fox-onboarding-form">
          <div className="fox-onboarding-card">
            <div className="fox-onboarding-progress" aria-label={`步骤 ${step + 1} / ${steps.length}`}><span>步骤 {step + 1} / {steps.length}</span><div>{steps.map((item, index) => <i key={item.title} className={index <= step ? 'is-active' : ''} />)}</div></div>
            <div className="fox-onboarding-step-viewport">
              <AnimatePresence initial={false} mode="sync" custom={stepDirection}>
                <motion.div
                  key={step}
                  className="fox-onboarding-step-content"
                  custom={stepDirection}
                  initial={{ x: stepDirection >= 0 ? '-100%' : '100%', opacity: 0 }}
                  animate={{ x: '0%', opacity: 1 }}
                  exit={{ x: stepDirection >= 0 ? '50%' : '-50%', opacity: 0 }}
                  transition={{ duration: 0.4, ease: [0.22, 1, 0.36, 1] }}
                >
                  <header><h1>{current.title}</h1><p>{current.description}</p></header>
                  <div className={`fox-onboarding-status ${current.ready ? 'is-ready' : ''}`}><span>{current.ready ? <Check /> : <LoaderCircle />}</span><div><b>{current.ready ? '当前状态正常' : '尚未完成'}</b><small>{statusCopy}</small></div><Badge variant={current.ready ? 'secondary' : 'outline'}>{current.ready ? '已就绪' : '待配置'}</Badge></div>
                  {step < 2 && <Button variant="outline" className="fox-onboarding-configure" onClick={current.action}>{current.actionLabel}<ChevronRight /></Button>}
                </motion.div>
              </AnimatePresence>
            </div>
            <footer><Button variant="outline" className="fox-onboarding-prev" disabled={step === 0} onClick={goBack}>上一步</Button><BorderBeam active={step === 2 && Boolean(model.service)} className="fox-onboarding-next-beam" colorVariant="colorful" size="md" strength={0.7} theme={dark ? 'dark' : 'light'}><Button className="fox-onboarding-next" disabled={step === 2 && !model.service} onClick={() => step === 2 ? current.action() : advance()}>{step === 2 ? '进入 Fox' : '下一步'}<ChevronRight /></Button></BorderBeam></footer>
            {step === 2 && !model.service && <button type="button" className="fox-onboarding-skip-link" onClick={() => onExit('skipped')}>暂时跳过，稍后在设置中配置</button>}
          </div>
        </main>
      </section>
      <aside className="fox-onboarding-visual">
        <div className="fox-onboarding-visual-media"><img src="/mascot/fox/status/fox_sayhi.png" alt="Fox 卡通形象" /><div><strong>Fox Desktop</strong><span>轻量、高效的桌面专家工作台</span></div><BorderBeam className="fox-onboarding-enter" colorVariant="colorful" size="md" strength={0.7} theme="light"><Button className="fox-onboarding-enter-button" size="sm" variant="outline" onClick={() => onExit(model.service ? 'completed' : 'skipped')}>进入 Fox<ChevronRight /></Button></BorderBeam></div>
      </aside>
    </div>
  </div>
}

type GeneralAssistantDraft = {
  name: string
  description: string
  systemPrompt: string
  openingSuggestions: string
}

export function AiSettingsPage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const model = useModelService()
  const providers = useModelProviders()
  const [assistant, setAssistant] = useState<AgentRecord | null>(null)
  const [assistantLoading, setAssistantLoading] = useState(desktopRuntimeAvailable)
  const [assistantEditorOpen, setAssistantEditorOpen] = useState(false)
  const [assistantSaving, setAssistantSaving] = useState(false)
  const [assistantDraft, setAssistantDraft] = useState<GeneralAssistantDraft | null>(null)
  const selected = providers.items.flatMap((provider) => provider.models.map((item) => ({ provider, item }))).find(({ provider, item }) => provider.isDefault && item.isDefault)

  useEffect(() => {
    let cancelled = false
    if (!desktopRuntimeAvailable) {
      setAssistantLoading(false)
      return
    }
    void desktopClient.listAgents()
      .then((items) => {
        if (!cancelled) setAssistant(items.find((item) => item.id === 'fox-general') ?? null)
      })
      .catch((cause) => {
        if (!cancelled) toast.error('通用助手配置读取失败', { description: desktopErrorDetails(cause).message })
      })
      .finally(() => {
        if (!cancelled) setAssistantLoading(false)
      })
    return () => { cancelled = true }
  }, [])

  const selectModel = async (value: string) => {
    const [providerId, modelId] = value.split('::')
    const provider = providers.items.find((item) => item.id === providerId)
    if (!provider) return
    const saved = await providers.save({ id: provider.id, name: provider.name, icon: provider.icon, baseUrl: provider.baseUrl, apiType: provider.apiType, isDefault: true, models: provider.models.map((item) => ({ ...item, isDefault: item.modelId === modelId })) })
    if (saved) { await model.refresh(); toast.success('默认模型已更新') }
  }

  const openAssistantEditor = () => {
    if (!assistant) return
    setAssistantDraft({
      name: assistant.name,
      description: assistant.description,
      systemPrompt: assistant.systemPrompt,
      openingSuggestions: assistant.openingSuggestions.join('\n'),
    })
    setAssistantEditorOpen(true)
  }

  const saveAssistant = async () => {
    if (!assistant || !assistantDraft) return
    if (!assistantDraft.name.trim() || !assistantDraft.systemPrompt.trim()) {
      toast.error('请填写助手名称和系统提示词')
      return
    }
    setAssistantSaving(true)
    try {
      const updated = await desktopClient.saveAgent({
        id: assistant.id,
        name: assistantDraft.name.trim(),
        description: assistantDraft.description.trim(),
        icon: assistant.icon,
        category: assistant.category,
        systemPrompt: assistantDraft.systemPrompt.trim(),
        defaultModel: assistant.defaultModel,
        openingSuggestions: assistantDraft.openingSuggestions.split(/\r?\n/).map((item) => item.trim()).filter(Boolean).slice(0, 6),
        packageManifest: assistant.packageManifest,
      })
      setAssistant(updated)
      setAssistantEditorOpen(false)
      toast.success('Fox 通用助手设置已保存')
    } catch (cause) {
      toast.error('保存通用助手设置失败', { description: desktopErrorDetails(cause).message })
    } finally {
      setAssistantSaving(false)
    }
  }

  return (
    <WorkspacePage title="助手设置" subtitle="Fox Runtime" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
      <SettingsScaffold kicker="运行默认值" title="助手设置" description="配置 Fox 通用助手的基础行为与新对话默认模型。">
        <Card className="fox-settings-section">
          <div className="fox-settings-section-head"><div><h2>Fox 通用助手</h2></div><Button variant="outline" size="sm" disabled={!assistant || assistantLoading} onClick={openAssistantEditor}><Pencil />编辑助手</Button></div>
          <div className="fox-assistant-settings-summary">
            <span>{assistant?.icon ? <img src={assistant.icon} alt="" /> : assistantLoading ? <LoaderCircle className="animate-spin" /> : <Bot />}</span>
            <div><b>{assistant?.name ?? (assistantLoading ? '正在读取助手设置…' : '暂时无法读取通用助手')}</b><small>{assistant?.description || '可调整名称、简介、系统提示词和新对话开场建议。'}</small></div>
          </div>
        </Card>
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>模型偏好</h2></div></div><PreferenceRow title="默认模型" description={selected ? `${selected.provider.name} · ${selected.item.displayName}` : '尚未配置可用模型'}>{providers.items.length ? <Select value={selected ? `${selected.provider.id}::${selected.item.modelId}` : undefined} onValueChange={(value) => void selectModel(value)}><SelectTrigger className="fox-settings-select fox-model-select-adaptive"><SelectValue placeholder="选择默认模型" /></SelectTrigger><SelectContent>{providers.items.flatMap((provider) => provider.models.map((item) => <SelectItem key={`${provider.id}:${item.id}`} value={`${provider.id}::${item.modelId}`}>{provider.name} · {item.displayName}</SelectItem>))}</SelectContent></Select> : <Button variant="outline" size="sm" onClick={() => navigate('settings-models')}>配置供应商</Button>}</PreferenceRow></Card>
        {model.service && <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>当前模型</h2></div><Badge variant="secondary">{serviceStatusLabel(model.service.lastStatus)}</Badge></div><div className="fox-property-list fox-current-model-properties"><div><span>供应商</span><b>{model.service.name}</b></div><div><span>协议</span><b>{model.service.apiType === 'anthropic-messages' ? 'Anthropic Messages' : 'OpenAI-compatible'}</b></div><div><span>上下文</span><b>{model.service.contextWindow.toLocaleString()} tokens</b></div><div><span>多模态</span><b>{model.service.supportsImageInput ? '支持图片' : '仅文本'}</b></div></div></Card>}
      </SettingsScaffold>
      <Dialog open={assistantEditorOpen} onOpenChange={setAssistantEditorOpen}>
        <DialogContent className="fox-assistant-settings-dialog">
          <DialogHeader><DialogTitle>Fox 通用助手</DialogTitle><DialogDescription>这些设置会应用于之后由通用助手创建的新对话，不改变专家和远程助手。</DialogDescription></DialogHeader>
          {assistantDraft && <div className="fox-assistant-settings-form">
            <div><label><span>助手名称</span><Input value={assistantDraft.name} onChange={(event) => setAssistantDraft((value) => value ? { ...value, name: event.target.value } : value)} /></label><label><span>助手简介</span><Input value={assistantDraft.description} onChange={(event) => setAssistantDraft((value) => value ? { ...value, description: event.target.value } : value)} /></label></div>
            <label><span>系统提示词</span><Textarea className="fox-assistant-prompt-input" value={assistantDraft.systemPrompt} onChange={(event) => setAssistantDraft((value) => value ? { ...value, systemPrompt: event.target.value } : value)} /></label>
            <label><span>开场建议（每行一条，最多 6 条）</span><Textarea className="fox-assistant-suggestions-input" value={assistantDraft.openingSuggestions} onChange={(event) => setAssistantDraft((value) => value ? { ...value, openingSuggestions: event.target.value } : value)} /></label>
          </div>}
          <DialogFooter><Button variant="outline" disabled={assistantSaving} onClick={() => setAssistantEditorOpen(false)}>取消</Button><Button disabled={assistantSaving || !assistantDraft?.name.trim() || !assistantDraft?.systemPrompt.trim()} onClick={() => void saveAssistant()}>{assistantSaving && <LoaderCircle className="animate-spin" />}保存</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </WorkspacePage>
  )
}

type ProviderForm = SaveModelProviderInput & { apiKey: string; clearApiKey: boolean }

type ProviderPreset = {
  id: string
  name: string
  icon: string
  baseUrl: string
  apiType: ProviderForm['apiType']
  modelPlaceholder: string
  contextWindow: number
  maxOutputTokens: number
  supportsImageInput: boolean
}

// Keep these defaults aligned with Yuxi's builtin provider catalog.
const providerPresets: ProviderPreset[] = [
  { id: 'openai', name: 'OpenAI', icon: 'openai.svg', baseUrl: 'https://api.openai.com/v1', apiType: 'openai-completions', modelPlaceholder: 'gpt-4.1', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'deepseek', name: 'DeepSeek', icon: 'deepseek.svg', baseUrl: 'https://api.deepseek.com', apiType: 'openai-completions', modelPlaceholder: 'deepseek-chat', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false },
  { id: 'alibaba', name: 'DashScope', icon: 'alibaba.svg', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', apiType: 'openai-completions', modelPlaceholder: 'qwen3-max', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: true },
  { id: 'alibaba-coding-plan-cn', name: 'Aliyun Coding Plan', icon: 'alibaba-cloud.svg', baseUrl: 'https://coding.dashscope.aliyuncs.com/v1', apiType: 'openai-completions', modelPlaceholder: 'qwen3-coder-plus', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'alibaba-coding-plan', name: 'Aliyun Coding Plan (International)', icon: 'alibaba-cloud.svg', baseUrl: 'https://coding-intl.dashscope.aliyuncs.com/v1', apiType: 'openai-completions', modelPlaceholder: 'qwen3-coder-plus', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'zhipuai', name: 'Zhipu (BigModel)', icon: 'zhipu.svg', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', apiType: 'openai-completions', modelPlaceholder: 'glm-4.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'zhipuai-coding-plan', name: 'Zhipu Coding Plan (BigModel)', icon: 'zhipu.svg', baseUrl: 'https://open.bigmodel.cn/api/coding/paas/v4', apiType: 'openai-completions', modelPlaceholder: 'glm-4.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'zai', name: 'Zhipu (Z.AI)', icon: 'zai.svg', baseUrl: 'https://api.z.ai/api/paas/v4', apiType: 'openai-completions', modelPlaceholder: 'glm-4.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'zai-coding-plan', name: 'Zhipu Coding Plan (Z.AI)', icon: 'zai.svg', baseUrl: 'https://api.z.ai/api/coding/paas/v4', apiType: 'openai-completions', modelPlaceholder: 'glm-4.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'xiaomi-token-plan-cn', name: 'XiaomiMiMo Token Plan', icon: 'xiaomi.svg', baseUrl: 'https://token-plan-cn.xiaomimimo.com/v1', apiType: 'openai-completions', modelPlaceholder: 'mimo-v2-flash', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'xiaomi', name: 'XiaomiMiMo', icon: 'xiaomi.svg', baseUrl: 'https://api.xiaomimimo.com/v1', apiType: 'openai-completions', modelPlaceholder: 'mimo-v2-flash', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'kimi-for-coding', name: 'Kimi Code', icon: 'moonshot.svg', baseUrl: 'https://api.kimi.com/coding/v1', apiType: 'openai-completions', modelPlaceholder: 'kimi-for-coding', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'moonshotai-cn', name: 'Moonshot', icon: 'moonshot.svg', baseUrl: 'https://api.moonshot.cn/v1', apiType: 'openai-completions', modelPlaceholder: 'kimi-k2.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'moonshotai', name: 'Moonshot (International)', icon: 'moonshot.svg', baseUrl: 'https://api.moonshot.ai/v1', apiType: 'openai-completions', modelPlaceholder: 'kimi-k2.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'minimax-cn', name: 'MiniMax', icon: 'minimax.svg', baseUrl: 'https://api.minimaxi.com/v1', apiType: 'openai-completions', modelPlaceholder: 'MiniMax-M2.5', contextWindow: 204800, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'minimax', name: 'MiniMax (International)', icon: 'minimax.svg', baseUrl: 'https://api.minimax.io/v1', apiType: 'openai-completions', modelPlaceholder: 'MiniMax-M2.5', contextWindow: 204800, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'openrouter', name: 'OpenRouter', icon: 'openrouter.svg', baseUrl: 'https://openrouter.ai/api/v1', apiType: 'openai-completions', modelPlaceholder: 'openai/gpt-4.1', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'modelscope', name: 'ModelScope', icon: 'modelscope.svg', baseUrl: 'https://api-inference.modelscope.cn/v1', apiType: 'openai-completions', modelPlaceholder: 'Qwen/Qwen3-235B-A22B', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false },
  { id: 'opencode', name: 'OpenCode', icon: 'opencode.svg', baseUrl: 'https://opencode.ai/zen/v1', apiType: 'openai-completions', modelPlaceholder: 'opencode/big-pickle', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'siliconflow-cn', name: 'SiliconFlow', icon: 'siliconflow.svg', baseUrl: 'https://api.siliconflow.cn/v1', apiType: 'openai-completions', modelPlaceholder: 'deepseek-ai/DeepSeek-V4-Flash', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false },
  { id: 'siliconflow', name: 'SiliconFlow (International)', icon: 'siliconflow.svg', baseUrl: 'https://api.siliconflow.com/v1', apiType: 'openai-completions', modelPlaceholder: 'deepseek-ai/DeepSeek-V4-Flash', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false },
]

const providerIconOptions = Array.from(new Map(providerPresets.map((preset) => [preset.icon, preset])).values())

function providerIcon(name: string, baseUrl: string, customIcon?: string | null) {
  if (customIcon) return `/providers/${customIcon}`
  const preset = providerPresets.find((item) => item.baseUrl === baseUrl)
    ?? providerPresets.find((item) => name.toLowerCase().includes(item.id.split('-')[0]))
  return preset ? `/providers/${preset.icon}` : null
}

function providerPresetForm(preset: ProviderPreset, isDefault: boolean): ProviderForm {
  return {
    name: preset.name,
    icon: preset.icon,
    baseUrl: preset.baseUrl,
    apiType: preset.apiType,
    isDefault,
    apiKey: '',
    clearApiKey: false,
    models: [{
      modelId: '',
      displayName: '',
      contextWindow: preset.contextWindow,
      maxOutputTokens: preset.maxOutputTokens,
      supportsImageInput: preset.supportsImageInput,
      isDefault: true,
    }],
  }
}

function providerForm(provider?: ModelProviderRecord): ProviderForm {
  return provider ? {
    id: provider.id, name: provider.name, icon: provider.icon, baseUrl: provider.baseUrl, apiType: provider.apiType,
    isDefault: provider.isDefault, apiKey: '', clearApiKey: false,
    models: provider.models.map((item) => ({ ...item })),
  } : {
    name: '', icon: null, baseUrl: '', apiType: 'openai-completions', isDefault: true, apiKey: '', clearApiKey: false,
    models: [{ modelId: '', displayName: '', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false, isDefault: true }],
  }
}

function ProviderIconPicker({ form, onChange }: { form: ProviderForm; onChange: (form: ProviderForm) => void }) {
  const [open, setOpen] = useState(false)
  const selectedOption = providerIconOptions.find((option) => option.icon === form.icon)
  const resolvedIcon = providerIcon(form.name, form.baseUrl, form.icon)
  const selectIcon = (icon: string | null) => {
    onChange({ ...form, icon })
    setOpen(false)
  }

  return (
    <div className="fox-provider-form-field fox-provider-icon-select">
      <span>供应商图标</span>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button type="button" className="fox-provider-icon-trigger" aria-label="选择供应商图标" aria-expanded={open}>
            <span className="fox-provider-icon-preview">{resolvedIcon ? <img src={resolvedIcon} alt="" /> : <Server />}</span>
            <span>{form.icon ? selectedOption?.name ?? '自定义图标' : '自动识别'}</span>
            <ChevronDown aria-hidden="true" />
          </button>
        </PopoverTrigger>
        <PopoverContent align="end" sideOffset={6} className="fox-provider-icon-menu">
          <div className="fox-provider-icon-grid" role="radiogroup" aria-label="供应商图标">
            <button type="button" role="radio" aria-checked={!form.icon} className={!form.icon ? 'is-active' : ''} title="自动识别" onClick={() => selectIcon(null)}>
              <Server />
            </button>
            {providerIconOptions.map((option) => (
              <button type="button" role="radio" aria-checked={form.icon === option.icon} key={option.icon} className={form.icon === option.icon ? 'is-active' : ''} title={option.name} onClick={() => selectIcon(option.icon)}>
                <img src={`/providers/${option.icon}`} alt="" />
              </button>
            ))}
          </div>
          <p>自动识别会根据供应商名称和 API 地址匹配图标。</p>
        </PopoverContent>
      </Popover>
    </div>
  )
}

function ProviderEditor({ form, busy, testing = false, connectionStatus, credentialConfigured, error, onChange, onCancel, onSave, onDelete, onTest }: { form: ProviderForm; busy: boolean; testing?: boolean; connectionStatus?: string | null; credentialConfigured: boolean; error?: string | null; onChange: (form: ProviderForm) => void; onCancel: () => void; onSave: () => void; onDelete?: () => void; onTest: () => void }) {
  const updateModel = (index: number, patch: Partial<ProviderForm['models'][number]>) => onChange({ ...form, models: form.models.map((item, itemIndex) => itemIndex === index ? { ...item, ...patch } : patch.isDefault ? { ...item, isDefault: false } : item) })
  return <div className="fox-provider-editor"><div className="fox-provider-form-grid"><label>供应商名称<Input value={form.name} onChange={(event) => onChange({ ...form, name: event.target.value })} placeholder="例如 MiniMax" /></label><label>API 协议<Select value={form.apiType} onValueChange={(apiType) => onChange({ ...form, apiType: apiType as ProviderForm['apiType'] })}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="openai-completions">OpenAI-compatible</SelectItem><SelectItem value="anthropic-messages">Anthropic Messages</SelectItem></SelectContent></Select></label><ProviderIconPicker form={form} onChange={onChange} /><label className="is-wide">API 地址<Input value={form.baseUrl} onChange={(event) => onChange({ ...form, baseUrl: event.target.value })} placeholder={form.apiType === 'anthropic-messages' ? 'https://api.minimaxi.com/anthropic' : 'https://api.example.com/v1'} /></label><label className="is-wide">API Key<Input type="password" value={form.apiKey} onChange={(event) => onChange({ ...form, apiKey: event.target.value, clearApiKey: false })} placeholder={credentialConfigured ? '已安全保存，留空保持不变' : '输入供应商 API Key'} /></label></div><label className="fox-provider-default-toggle"><span><b>设为默认供应商</b><small>新对话默认使用此供应商中标记为默认的模型</small></span><Switch checked={form.isDefault} onCheckedChange={(isDefault) => onChange({ ...form, isDefault })} /></label><div className="fox-provider-models-head"><div><h3>模型</h3><p>一个供应商可以配置多个模型，并为每个模型设置独立能力。</p></div><Button variant="outline" size="sm" onClick={() => onChange({ ...form, models: [...form.models, { modelId: '', displayName: '', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false, isDefault: false }] })}><Plus />添加模型</Button></div><div className="fox-provider-model-list">{form.models.map((model, index) => <div className="fox-provider-model-row" key={model.id ?? index}><label>模型 ID<Input value={model.modelId} onChange={(event) => updateModel(index, { modelId: event.target.value })} placeholder="例如 MiniMax-M3" /></label><label>显示名称<Input value={model.displayName} onChange={(event) => updateModel(index, { displayName: event.target.value })} placeholder="留空则使用模型 ID" /></label><label>上下文<Input inputMode="numeric" value={model.contextWindow} onChange={(event) => updateModel(index, { contextWindow: Number(event.target.value) || 0 })} /></label><label>最大输出<Input inputMode="numeric" value={model.maxOutputTokens} onChange={(event) => updateModel(index, { maxOutputTokens: Number(event.target.value) || 0 })} /></label><label className="fox-provider-model-toggle"><span>多模态</span><Switch checked={model.supportsImageInput} onCheckedChange={(supportsImageInput) => updateModel(index, { supportsImageInput })} /></label><label className="fox-provider-model-toggle"><span>默认</span><Switch checked={model.isDefault} onCheckedChange={(isDefault) => updateModel(index, { isDefault })} /></label><Button variant="ghost" size="icon-sm" disabled={form.models.length === 1} title="删除模型" onClick={() => onChange({ ...form, models: form.models.filter((_, itemIndex) => itemIndex !== index) })}><Trash2 /></Button></div>)}</div>{form.id && credentialConfigured && <label className="fox-provider-clear-key"><Switch checked={form.clearApiKey} onCheckedChange={(clearApiKey) => onChange({ ...form, clearApiKey, apiKey: clearApiKey ? '' : form.apiKey })} /><span>保存时清除已存 API Key</span></label>}{error && <p className="fox-setting-error" role="alert">{error}</p>}{connectionStatus && <p className="fox-provider-test-status" role="status">{connectionStatus}</p>}<div className="fox-provider-editor-actions">{onDelete && <Button variant="ghost" className="is-danger" disabled={busy || form.isDefault} onClick={onDelete}><Trash2 />删除供应商</Button>}<span /><Button variant="outline" disabled={busy} onClick={onCancel}>取消</Button><Button variant="outline" disabled={busy || !form.baseUrl.trim() || !form.models[0]?.modelId.trim()} onClick={onTest}>{testing ? <LoaderCircle className="animate-spin" /> : <Zap />}{testing ? '正在验证…' : '验证连接'}</Button><Button disabled={busy || !form.name.trim() || !form.baseUrl.trim() || form.models.some((item) => !item.modelId.trim())} onClick={onSave}>{busy && <LoaderCircle className="animate-spin" />}保存</Button></div></div>
}

export function ModelProvidersPage({ sidebarCollapsed, onSidebar }: { sidebarCollapsed: boolean; onSidebar: () => void }) {
  const resource = useModelProviders()
  const [testing, setTesting] = useState(false)
  const [connectionStatus, setConnectionStatus] = useState<string | null>(null)
  const [editingId, setEditingId] = useState<string | 'new' | null>(null)
  const [form, setForm] = useState<ProviderForm | null>(null)
  const openEditor = (provider?: ModelProviderRecord) => { setConnectionStatus(null); const next = providerForm(provider); if (!provider) next.isDefault = resource.items.length === 0; setEditingId(provider?.id ?? 'new'); setForm(next) }
  const openPreset = (preset: ProviderPreset) => { setConnectionStatus(null); setEditingId('new'); setForm(providerPresetForm(preset, resource.items.length === 0)) }
  const unconfiguredPresets = providerPresets.filter((preset) => !resource.items.some((provider) => provider.baseUrl.replace(/\/$/, '') === preset.baseUrl.replace(/\/$/, '')))
  const save = async () => {
    if (!form) return
    const saved = await resource.save({ ...form, apiKey: form.apiKey || undefined })
    if (saved) { setEditingId(null); setForm(null); toast.success('模型供应商已保存') }
  }
  const test = async () => {
    if (!form || testing) return
    const model = form.models.find((item) => item.isDefault) ?? form.models[0]
    setTesting(true)
    setConnectionStatus('正在验证连接…')
    try {
      const result = await desktopClient.testModelService(form.baseUrl, form.apiKey || undefined, form.apiType, model?.modelId)
      setConnectionStatus(`连接成功，发现 ${result.models.length} 个模型`)
    } catch (cause) { setConnectionStatus(`连接失败：${desktopErrorDetails(cause).message}`) }
    finally { setTesting(false) }
  }
  const remove = async (provider: ModelProviderRecord) => {
    if (!window.confirm(`删除模型供应商“${provider.name}”？`)) return
    if (await resource.remove(provider.id)) { setEditingId(null); setForm(null); toast.success('供应商已删除') }
  }
  const rename = async (provider: ModelProviderRecord) => {
    const name = window.prompt('供应商名称', provider.name)?.trim()
    if (!name || name === provider.name) return
    const saved = await resource.save({ id: provider.id, name, icon: provider.icon, baseUrl: provider.baseUrl, apiType: provider.apiType, isDefault: provider.isDefault, models: provider.models })
    if (saved) toast.success('供应商已重命名')
  }
  return (
    <WorkspacePage title="模型供应商" subtitle="API 与模型连接" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
      <SettingsScaffold kicker="模型连接" title="模型供应商" description="管理 Fox 原生 Agent 可使用的模型供应商、API 凭证和模型能力。">
        <div className="fox-provider-toolbar">
          <div>
            <b>{resource.items.length} 个已配置供应商</b>
            <small>{resource.items.reduce((sum, item) => sum + item.models.length, 0)} 个可用模型 · {unconfiguredPresets.length} 个内置供应商待配置</small>
          </div>
          <Button size="sm" onClick={() => openEditor()}><Plus />添加供应商</Button>
        </div>
        <Dialog open={editingId === 'new'} onOpenChange={(open) => { if (!open) { setEditingId(null); setForm(null) } }}>
          <DialogContent className="fox-provider-dialog">
            <DialogHeader><DialogTitle>添加模型供应商</DialogTitle><DialogDescription>配置 API 连接和 Fox 可以使用的模型。保存后仍可随时编辑。</DialogDescription></DialogHeader>
            {editingId === 'new' && form && <ProviderEditor form={form} busy={testing || resource.busyId === 'new'} credentialConfigured={false} testing={testing} connectionStatus={connectionStatus} error={resource.error} onChange={(next) => { setConnectionStatus(null); setForm(next) }} onCancel={() => { setEditingId(null); setForm(null) }} onSave={() => void save()} onTest={() => void test()} />}
          </DialogContent>
        </Dialog>
        {resource.loading && <Card className="fox-settings-section"><LoaderCircle className="animate-spin" />正在读取供应商…</Card>}
        <div className="fox-provider-list">
          {resource.items.map((provider) => {
            const icon = providerIcon(provider.name, provider.baseUrl, provider.icon)
            return <div className="fox-provider-entry" key={provider.id}>
              <Card className="fox-provider-card">
                <div className="fox-provider-mark">{icon ? <img src={icon} alt="" /> : <Server />}</div>
                <div className="fox-provider-card-main">
                  <div><h2>{provider.name}</h2>{provider.isDefault && <Badge variant="secondary">默认</Badge>}<Badge variant="outline">{provider.apiType === 'anthropic-messages' ? 'Anthropic' : 'OpenAI'}</Badge></div>
                  <p>{provider.baseUrl}</p>
                  <div className="fox-provider-model-tags">{provider.models.slice(0, 4).map((model) => <Badge key={model.id} variant="secondary">{model.displayName}{model.supportsImageInput ? ' · 多模态' : ''}</Badge>)}{provider.models.length > 4 && <span>+{provider.models.length - 4}</span>}</div>
                </div>
                <div className="fox-provider-card-status"><i className={provider.lastStatus === 'connected' ? '' : 'is-muted'} /><span><b>{serviceStatusLabel(provider.lastStatus)}</b><small>{provider.lastLatencyMs != null ? `${provider.lastLatencyMs} ms` : `${provider.models.length} 个模型`}</small></span></div>
                <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-provider-more" aria-label={`管理 ${provider.name}`}><MoreHorizontal size={18} /></Button></DropdownMenuTrigger><DropdownMenuContent align="end"><DropdownMenuItem onSelect={() => void rename(provider)}><Pencil />重命名</DropdownMenuItem><DropdownMenuItem onSelect={() => openEditor(provider)}><Wrench />编辑</DropdownMenuItem><DropdownMenuItem onSelect={() => void desktopClient.testModelService(provider.baseUrl, undefined, provider.apiType, provider.models.find((item) => item.isDefault)?.modelId ?? provider.models[0]?.modelId).then((result) => { void resource.refresh(); toast.success(`连接成功，发现 ${result.models.length} 个模型`) }).catch((cause) => toast.error(cause instanceof Error ? cause.message : String(cause)))}><Zap />验证连接</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem variant="destructive" disabled={provider.isDefault} onSelect={() => void remove(provider)}><Trash2 />删除</DropdownMenuItem></DropdownMenuContent></DropdownMenu>
              </Card>
              {editingId === provider.id && form && <ProviderEditor form={form} busy={testing || resource.busyId === provider.id} credentialConfigured={provider.credentialConfigured} testing={testing} connectionStatus={connectionStatus} error={resource.error} onChange={(next) => { setConnectionStatus(null); setForm(next) }} onCancel={() => { setEditingId(null); setForm(null) }} onSave={() => void save()} onDelete={() => void remove(provider)} onTest={() => void test()} />}
            </div>
          })}
          {!resource.loading && unconfiguredPresets.map((preset) => <div className="fox-provider-entry" key={preset.id}>
            <Card className="fox-provider-card fox-provider-card-preset" onClick={() => openPreset(preset)}>
              <div className="fox-provider-mark"><img src={`/providers/${preset.icon}`} alt="" /></div>
              <div className="fox-provider-card-main">
                <div><h2>{preset.name}</h2><Badge variant="secondary">内置</Badge><Badge variant="outline">OpenAI</Badge></div>
                <p>{preset.baseUrl}</p>
                <div className="fox-provider-model-tags"><Badge variant="secondary">{preset.modelPlaceholder}</Badge></div>
              </div>
              <div className="fox-provider-card-status"><i className="is-muted" /><span><b>未配置</b><small>填写 API Key 与默认模型</small></span></div>
              <Button variant="ghost" size="icon" className="fox-provider-more" aria-label={`配置 ${preset.name}`} onClick={(event) => { event.stopPropagation(); openPreset(preset) }}><MoreHorizontal size={18} /></Button>
            </Card>
          </div>)}
        </div>
      </SettingsScaffold>
    </WorkspacePage>
  )
}

export function YuxiSettingsPage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const yuxi = useYuxiService()
  const user = useYuxiUser(Boolean(yuxi.service?.credentialConfigured))
  const openService = () => {
    window.sessionStorage.setItem(YUXI_SERVICE_RETURN_KEY, 'settings-yuxi')
    navigate('service')
  }
  const openLogin = () => {
    window.sessionStorage.setItem(YUXI_LOGIN_RETURN_KEY, 'settings-yuxi')
    navigate('login')
  }
  return <WorkspacePage title="知识库服务" subtitle="知识与远程专家" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}><SettingsScaffold kicker="服务连接" title="知识库服务" description="管理 Fox 使用的唯一知识库服务和账户身份。"><Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>服务连接</h2><p>支持本机、局域网和远程 HTTPS 地址。</p></div><Badge variant="secondary">{yuxi.service ? serviceStatusLabel(yuxi.service.lastStatus) : '未配置'}</Badge></div><button className="fox-service-card" onClick={openService}><span><Server /></span><p><b>{yuxi.service ? knowledgeServiceName(yuxi.service.name) : '配置知识库服务'}</b><small>{yuxi.service?.baseUrl ?? '专家、知识库和知识图谱能力的来源'}</small></p><em><b>{yuxi.service?.lastLatencyMs != null ? `${yuxi.service.lastLatencyMs} ms` : '--'}</b><small>{connectionTypeLabel(yuxi.service?.connectionType)}</small></em><ChevronRight /></button><div className="fox-setting-actions"><Button variant="outline" size="sm" disabled={!yuxi.service || yuxi.testing} onClick={() => void yuxi.test()}>{yuxi.testing && <LoaderCircle className="animate-spin" />}测试连接</Button><Button size="sm" onClick={openService}>{yuxi.service ? '编辑地址' : '开始配置'}</Button></div></Card><Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>账户</h2><p>Fox 与浏览器中的知识库登录态相互独立。</p></div>{user.user ? <Badge variant="secondary">已登录</Badge> : <Badge variant="outline">未登录</Badge>}</div>{user.user ? <div className="fox-profile-setting"><Avatar>{user.user.avatar && <AvatarImage src={user.user.avatar} alt="" />}<AvatarFallback>{user.user.username.charAt(0).toUpperCase()}</AvatarFallback></Avatar><span><b>{user.user.username}</b><small>{user.user.uid}</small><em>{[user.user.departmentName, user.user.role].filter(Boolean).join(' · ')}</em></span></div> : <Button variant="outline" onClick={openLogin}><LogIn />知识库登录</Button>}</Card></SettingsScaffold></WorkspacePage>
}

function projectStorageLabel(bytes: number | null) {
  if (bytes === null) return '未统计'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`
}

export function ProjectPermissionsPage({ sidebarCollapsed, onSidebar }: { sidebarCollapsed: boolean; onSidebar: () => void }) {
  const [projects, setProjects] = useState<ProjectManagementRecord[]>([])
  const [defaultPermission, setDefaultPermission] = useStoredPreference('fox.preferences.defaultPermission', 'ask')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const loadProjects = async () => {
    setLoading(true)
    try {
      setProjects(await desktopClient.listManagedProjects())
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => { void loadProjects() }, [])
  const updatePermission = async (project: ProjectManagementRecord, permissionMode: ProjectRecord['permissionMode']) => {
    setBusyId(project.id)
    try {
      const updated = await desktopClient.updateProjectPermission(project.id, permissionMode)
      setProjects((items) => items.map((item) => item.id === updated.id ? { ...item, permissionMode: updated.permissionMode, updatedAt: updated.updatedAt } : item))
      toast.success(`已更新 ${updated.name} 的执行方式`)
    } catch (cause) {
      toast.error('权限更新失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setBusyId(null)
    }
  }
  const chooseProjectPath = async (project: ProjectManagementRecord) => {
    setBusyId(project.id)
    try {
      const rootPath = await desktopClient.pickProjectFolder()
      if (!rootPath) return
      const updated = await desktopClient.updateProjectPath(project.id, rootPath)
      setProjects((items) => items.map((item) => item.id === updated.id ? updated : item))
      toast.success(`已迁移 ${updated.name} 的项目路径`)
    } catch (cause) {
      toast.error('项目路径更新失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setBusyId(null)
    }
  }
  const setArchived = async (project: ProjectManagementRecord, archived: boolean) => {
    setBusyId(project.id)
    try {
      await desktopClient.setProjectArchived(project.id, archived)
      setProjects((items) => items.map((item) => item.id === project.id ? { ...item, archivedAt: archived ? Date.now() : null } : item))
      toast.success(archived ? `已归档 ${project.name}` : `已恢复 ${project.name}`)
    } catch (cause) {
      toast.error(archived ? '项目归档失败' : '项目恢复失败', { description: cause instanceof Error ? cause.message : String(cause) })
    } finally {
      setBusyId(null)
    }
  }
  const openRoot = async (project: ProjectManagementRecord) => {
    try {
      await desktopClient.openProjectRoot(project.id)
    } catch (cause) {
      toast.error('无法打开项目目录', { description: cause instanceof Error ? cause.message : String(cause) })
    }
  }
  const activeProjects = projects.filter((project) => project.archivedAt === null)
  const archivedProjects = projects.filter((project) => project.archivedAt !== null)
  const renderProject = (project: ProjectManagementRecord) => <article key={project.id} className={`fox-managed-project ${project.pathExists ? '' : 'is-missing'} ${project.archivedAt ? 'is-archived' : ''}`}>
    <header>
      <span className="fox-managed-project-icon"><FolderOpen /></span>
      <span><strong>{project.name}</strong><small title={project.rootPath}>{project.rootPath}</small></span>
      <Badge variant={project.pathExists ? 'secondary' : 'destructive'}>{project.pathExists ? '路径正常' : '路径失效'}</Badge>
      {!project.archivedAt && <Button className="fox-managed-project-archive" variant="ghost" size="icon" disabled={busyId === project.id} aria-label={`归档 ${project.name}`} title="归档项目" onClick={() => void setArchived(project, true)}><Archive /></Button>}
    </header>
    <div className="fox-managed-project-metrics">
      <span><b>{project.isGitRepository ? project.gitBranch || 'Git' : '普通目录'}</b><small>版本状态</small></span>
      <span><b>{projectStorageLabel(project.diskBytes)}</b><small>目录占用</small></span>
      <span><b>{project.conversationCount}</b><small>关联会话</small></span>
      <span><b>{project.artifactCount}</b><small>产物</small></span>
    </div>
    <div className="fox-managed-project-latest"><span><MessageSquare /><small>最近对话</small><b>{project.recentConversationTitle ?? '暂无'}</b></span><span><FileText /><small>最近产物</small><b>{project.recentArtifactName ?? '暂无'}</b></span></div>
    <footer>
      {project.archivedAt
        ? <Button className="fox-managed-project-restore" variant="outline" size="sm" disabled={busyId === project.id} onClick={() => void setArchived(project, false)}><RotateCcw />恢复项目</Button>
        : <>
          <Select disabled={busyId === project.id} value={project.permissionMode} onValueChange={(value) => void updatePermission(project, value as ProjectRecord['permissionMode'])}>
            <SelectTrigger className="fox-settings-select" aria-label={`${project.name} 执行方式`}><SelectValue /></SelectTrigger>
            <SelectContent><SelectItem value="read_only">只读</SelectItem><SelectItem value="ask">询问</SelectItem><SelectItem value="allow">允许</SelectItem></SelectContent>
          </Select>
          <Button variant="outline" size="sm" disabled={!project.pathExists} onClick={() => void openRoot(project)}><ExternalLink />打开目录</Button>
          <Button variant="outline" size="sm" disabled={busyId === project.id} onClick={() => void chooseProjectPath(project)}>{busyId === project.id ? <LoaderCircle className="animate-spin" /> : <FolderOpen />}迁移目录</Button>
        </>}
    </footer>
  </article>
  return <WorkspacePage title="项目与权限" subtitle="本地文件边界" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}><SettingsScaffold kicker="安全边界" title="项目与权限" description="管理已授权文件夹、运行边界与项目生命周期。">
    <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>默认执行方式</h2><p>只影响之后加入的新项目；已有项目保持各自设置。</p></div><ShieldCheck /></div><PreferenceRow title="新项目默认权限" description="建议使用“询问”，在写入或运行命令前确认"><Select value={defaultPermission} onValueChange={setDefaultPermission}><SelectTrigger className="fox-settings-select"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="read_only">只读</SelectItem><SelectItem value="ask">询问</SelectItem><SelectItem value="allow">允许</SelectItem></SelectContent></Select></PreferenceRow></Card>
    <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>项目</h2><p>状态、路径与统计信息均来自本地运行时。</p></div><Button variant="outline" size="sm" disabled={loading} onClick={() => void loadProjects()}>{loading ? <LoaderCircle className="animate-spin" /> : <RotateCcw />}刷新</Button></div>{error ? <div className="fox-managed-project-state is-error"><FileWarning /><span><b>项目列表加载失败</b><small>{error}</small></span><Button variant="outline" size="sm" onClick={() => void loadProjects()}>重试</Button></div> : loading ? <div className="fox-managed-project-state"><LoaderCircle className="animate-spin" /><span><b>正在读取项目</b><small>正在检查目录、Git 状态与占用空间</small></span></div> : activeProjects.length ? <div className="fox-managed-project-grid">{activeProjects.map(renderProject)}</div> : <div className="fox-managed-project-state"><FolderOpen /><span><b>还没有项目</b><small>可从主界面的“添加项目”选择一个本地文件夹。</small></span></div>}</Card>
    {archivedProjects.length > 0 && <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>已归档</h2><p>归档项目不会出现在日常项目列表中，可以随时恢复。</p></div><Archive /></div><div className="fox-managed-project-grid">{archivedProjects.map(renderProject)}</div></Card>}
    <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>高风险操作</h2><p>命令、连接器工具和项目外写入仍然需要单独审批。</p></div><Shield /></div><PreferenceRow title="命令执行" description="当前阶段始终询问，后续再提供自动审批规则"><Badge className="fox-settings-status-pill is-wide" variant="secondary">始终询问</Badge></PreferenceRow><PreferenceRow title="知识库内容注入" description="知识检索结果不能绕过工具权限和项目边界"><Badge className="fox-settings-status-pill" variant="secondary">受保护</Badge></PreferenceRow></Card>
  </SettingsScaffold></WorkspacePage>
}

export function ConversationSettingsPage({ sidebarCollapsed, onSidebar }: { sidebarCollapsed: boolean; onSidebar: () => void }) {
  const [spellcheck, setSpellcheck] = useStoredPreference('fox.preferences.spellcheck', true)
  return <WorkspacePage title="输入与对话" subtitle="消息与历史记录" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}><SettingsScaffold kicker="交互偏好" title="输入与对话" description="调整发送消息与附件行为。"><Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>消息输入</h2><p>统一键盘输入行为。</p></div><Keyboard /></div><PreferenceRow title="发送消息" description="Enter 发送；中文输入法选字时 Enter 只确认候选字"><Badge className="fox-settings-status-pill" variant="secondary">Enter</Badge></PreferenceRow><PreferenceRow title="换行" description="Ctrl / Cmd + Enter 或 Shift + Enter 换行"><Badge className="fox-settings-status-pill" variant="secondary">Ctrl / Cmd + Enter</Badge></PreferenceRow><PreferenceRow title="拼写检查" description="使用系统 WebView 的拼写检查能力"><Switch checked={spellcheck} onCheckedChange={setSpellcheck} /></PreferenceRow></Card><Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>对话与附件</h2><p>新会话只在首次发送消息后写入历史记录。</p></div><MessageSquare /></div><PreferenceRow title="文档附件" description="文本与文档附件已支持；图片理解取决于当前模型"><Badge className="fox-settings-status-pill" variant="secondary">已启用</Badge></PreferenceRow><PreferenceRow title="历史回合导航" description="在对话左侧显示当前会话的消息缩略导航"><Badge className="fox-settings-status-pill" variant="secondary">已启用</Badge></PreferenceRow></Card></SettingsScaffold></WorkspacePage>
}

function formatUsageNumber(value: number) {
  return new Intl.NumberFormat('zh-CN', { notation: value >= 10_000 ? 'compact' : 'standard', maximumFractionDigits: 1 }).format(value)
}

function formatDuration(value: number | null) {
  if (value === null) return '进行中'
  if (value < 1000) return `${value} ms`
  return `${(value / 1000).toFixed(value < 10_000 ? 1 : 0)} s`
}

export function UsageStatisticsPage({ sidebarCollapsed, onSidebar }: { sidebarCollapsed: boolean; onSidebar: () => void }) {
  const [statistics, setStatistics] = useState<UsageStatistics | null>(null)
  const [observability, setObservability] = useState<ObservabilityStatistics | null>(null)
  const [loading, setLoading] = useState(true)
  const [evaluationLoading, setEvaluationLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const refresh = () => {
    setLoading(true)
    setError(null)
    void Promise.all([desktopClient.usageStatistics(), desktopClient.observabilityStatistics()])
      .then(([usage, traces]) => { setStatistics(usage); setObservability(traces) })
      .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
      .finally(() => setLoading(false))
  }
  const runEvaluation = () => {
    setEvaluationLoading(true)
    setError(null)
    void desktopClient.runOfflineEvaluation()
      .then((result) => {
        toast.success(`离线评测完成：${result.passed}/${result.total}`)
        refresh()
      })
      .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
      .finally(() => setEvaluationLoading(false))
  }
  useEffect(refresh, [])
  const activityRecord = useMemo(() => {
    const days = statistics?.days ?? []
    const leadingDays = days.length ? (new Date(`${days[0].date}T00:00:00`).getDay() + 6) % 7 : 0
    const months = days.reduce<string[]>((labels, day, index) => {
      const month = `${Number(day.date.slice(5, 7))}月`
      if (index === 0 || day.date.endsWith('-01')) labels.push(month)
      return labels
    }, [])
    return {
      days: [...Array.from({ length: leadingDays }, () => null), ...days],
      months,
      maxActivity: Math.max(1, ...days.map((day) => day.runCount + day.conversationCount)),
    }
  }, [statistics])
  const maxAgentRuns = Math.max(1, ...(statistics?.agents.map((agent) => agent.runCount) ?? [1]))
  const cacheEligibleTokens = (statistics?.inputTokens ?? 0) + (statistics?.cacheReadTokens ?? 0)
  const cacheHitRate = cacheEligibleTokens ? Math.round((statistics?.cacheReadTokens ?? 0) / cacheEligibleTokens * 100) : 0
  const latestEvaluation = observability?.evaluationHistory[0] ?? null
  const operationLabels: Record<string, string> = { plan: '规划', inference: '模型', execute_tool: '工具', host_prepare: '准备', finalize: '收尾', ui_render: '界面' }
  return <WorkspacePage title="使用统计" subtitle="活动、Trace 与评测" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}><SettingsScaffold kicker="本地统计" title="使用统计" description="查看 Fox 在这台设备上的使用、分段延迟、Trace 覆盖与固定离线回归；不包含费用估算。">
    {error && <p className="fox-setting-error">{error}</p>}
    {!statistics && loading ? <Card className="fox-settings-section fox-usage-loading"><LoaderCircle className="animate-spin" /><span>正在汇总使用记录</span></Card> : statistics && <>
      <Card className="fox-agent-work-card fox-usage-record-card">
        <div className="fox-agent-work-head"><h2>使用记录</h2><Button className="fox-usage-refresh" variant="outline" size="sm" onClick={refresh} disabled={loading}><RotateCcw className={loading ? 'animate-spin' : ''} />刷新</Button></div>
        <div className="fox-agent-work-content">
          <div className="fox-agent-work-metrics">
            {[['对话数', statistics.conversationCount], ['完成任务', statistics.completedRunCount], ['活跃天数', statistics.activeDayCount], ['Token 总量', formatUsageNumber(statistics.totalTokens)]].map(([label, value]) => <div key={label}><strong>{value}</strong><span>{label} <Info /></span></div>)}
          </div>
          <div className="fox-agent-activity-panel" aria-label="最近一年活动热力图">
            <div className="fox-agent-activity-months">{activityRecord.months.map((month, index) => <span key={`${month}-${index}`}>{month}</span>)}</div>
            <div className="fox-agent-activity-chart">
              <div className="fox-agent-activity-weekdays"><span>周一</span><span>周三</span><span>周五</span><span>周日</span></div>
              <div className="fox-agent-activity-grid">{activityRecord.days.map((day, index) => {
                if (!day) return <i key={`empty-${index}`} className="is-empty" aria-hidden="true" />
                const activity = day.runCount + day.conversationCount
                const level = activity === 0 ? 0 : Math.max(1, Math.ceil(activity / activityRecord.maxActivity * 4))
                return <i key={day.date} className={`is-level-${level}`} title={`${day.date} · ${day.conversationCount} 个对话 · ${day.runCount} 次运行 · ${formatUsageNumber(day.totalTokens)} tokens`} />
              })}</div>
            </div>
            <div className="fox-agent-activity-legend"><span>少</span>{[0, 1, 2, 3, 4].map((level) => <i key={level} className={`is-level-${level}`} />)}<span>多</span></div>
          </div>
        </div>
      </Card>
      <div className="fox-usage-columns">
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>专家使用</h2></div></div>{statistics.agents.length ? <div className="fox-usage-agents">{statistics.agents.map((agent, index) => <div key={agent.agentId}><span className="fox-usage-rank">{index + 1}</span><div><header><b>{agent.agentName}</b><small>{agent.runCount} 次运行</small></header><span><i style={{ width: `${agent.runCount / maxAgentRuns * 100}%` }} /></span><small>{agent.conversationCount} 个对话 · {formatUsageNumber(agent.totalTokens)} tokens</small></div></div>)}</div> : <p className="fox-settings-empty-copy">还没有专家使用记录。</p>}</Card>
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>Token 构成</h2><p>Prompt 与工具目录共享同一供应商缓存口径。</p></div><Badge variant="secondary">命中 {cacheHitRate}%</Badge></div><div className="fox-token-summary"><div><span>输入</span><strong>{formatUsageNumber(statistics.inputTokens)}</strong><i style={{ width: `${statistics.totalTokens ? statistics.inputTokens / statistics.totalTokens * 100 : 0}%` }} /></div><div><span>输出</span><strong>{formatUsageNumber(statistics.outputTokens)}</strong><i style={{ width: `${statistics.totalTokens ? statistics.outputTokens / statistics.totalTokens * 100 : 0}%` }} /></div><div><span>缓存读取</span><strong>{formatUsageNumber(statistics.cacheReadTokens)}</strong><i style={{ width: `${statistics.totalTokens ? statistics.cacheReadTokens / statistics.totalTokens * 100 : 0}%` }} /></div><div><span>缓存写入</span><strong>{formatUsageNumber(statistics.cacheWriteTokens)}</strong><i style={{ width: `${statistics.totalTokens ? statistics.cacheWriteTokens / statistics.totalTokens * 100 : 0}%` }} /></div></div><p className="fox-usage-note"><Activity />命中率 = 缓存读取 /（普通输入 + 缓存读取）；统计取每次运行最后一次 usage，避免流式重复累计。</p></Card>
      </div>
      {observability && <>
        <Card className="fox-settings-section fox-observability-card">
          <div className="fox-settings-section-head"><div><h2>运行可观测性</h2><p>Fox Trace schema v{observability.traceSchemaVersion} · 每次运行以 invoke_agent 为根 Span。</p></div><Badge variant={observability.traceCoveragePercent === 100 ? 'secondary' : 'outline'}>{observability.traceCoveragePercent}% Trace 覆盖</Badge></div>
          <div className={`fox-observability-metrics ${observability.latencyMetrics.length > 4 && observability.latencyMetrics.length % 3 === 0 ? 'is-balanced-three' : ''}`}>
            {observability.latencyMetrics.length ? observability.latencyMetrics.map((metric) => <div key={metric.operation}><span>{operationLabels[metric.operation] ?? metric.operation}</span><strong>P50 {formatDuration(metric.p50Ms)}</strong><small>P95 {formatDuration(metric.p95Ms)} · {metric.sampleCount} 样本</small></div>) : <p className="fox-settings-empty-copy">完成一次本地运行后会显示规划、模型、工具和界面分段耗时。</p>}
          </div>
          {observability.recentRuns.length > 0 && <div className="fox-trace-run-list">{observability.recentRuns.slice(0, 8).map((run) => <div key={run.runId}><span><b>{run.model || '默认模型'}</b><small title={run.traceId}>{run.traceId.slice(0, 12)}… · {run.spanCount} spans</small></span><span><small>规划 {formatDuration(run.planningDurationMs)} · 模型 {formatDuration(run.modelDurationMs)} · 工具 {formatDuration(run.toolDurationMs)}</small><b>{formatDuration(run.totalDurationMs)}</b></span></div>)}</div>}
        </Card>
        <Card className="fox-settings-section fox-evaluation-card">
          <div className="fox-settings-section-head"><div><h2>Agent 固定回归集</h2><p>SWE-bench、BFCL、AgentDojo 风格的本地契约评测；结果不冒充官方榜单成绩。</p></div><Button variant="outline" size="sm" onClick={runEvaluation} disabled={evaluationLoading || loading}>{evaluationLoading ? <LoaderCircle className="animate-spin" /> : <Zap />}运行离线评测</Button></div>
          {latestEvaluation ? <><div className="fox-evaluation-summary"><strong>{latestEvaluation.passed}/{latestEvaluation.total}</strong><span>{latestEvaluation.failed ? `${latestEvaluation.failed} 项失败` : '全部通过'} · {latestEvaluation.suites} 个套件 · {formatDuration(latestEvaluation.durationMs)}</span><code title={latestEvaluation.reportHash}>{latestEvaluation.reportHash.slice(0, 12)}</code></div><div className="fox-evaluation-suites">{latestEvaluation.suiteResults.map((suite) => <div key={suite.name}><span>{suite.name}</span><b className={suite.failed ? 'is-failed' : ''}>{suite.passed}/{suite.total}</b></div>)}</div>{observability.evaluationHistory.length > 1 && <p className="fox-usage-note"><Activity />已保留最近 {observability.evaluationHistory.length} 次结果，可观察固定数据集通过率趋势。</p>}</> : <p className="fox-settings-empty-copy">尚无本机评测记录。运行时只读取内置固定夹具，不调用真实模型、网络或项目文件。</p>}
        </Card>
      </>}
    </>}
  </SettingsScaffold></WorkspacePage>
}

export function ExtensionsSettingsPage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const skills = useSkills()
  const mcp = useMcpServers()
  return <WorkspacePage title="扩展" subtitle="技能与连接器" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}><SettingsScaffold kicker="能力扩展" title="扩展" description="统一从插件中心启停和管理技能、连接器。"><Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>技能</h2><p>为 Fox Agent 注入可审查的领域指令，不会自动增加工具权限。</p></div><Badge variant="secondary">{skills.items.filter((item) => item.enabled).length} 已启用</Badge></div><button className="fox-service-card" onClick={() => navigate('plugins', 'skill')}><span><Puzzle /></span><p><b>前往技能管理</b><small>{skills.items.length} 个已安装 · 与插件中心共用启停和配置入口</small></p><ChevronRight /></button></Card><Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>连接器</h2><p>连接本地进程、远程服务或 OpenAPI；所有调用仍经过 Fox 审批和审计。</p></div><Badge variant="secondary">{mcp.items.filter((item) => item.enabled).length} 已启用</Badge></div><button className="fox-service-card" onClick={() => navigate('plugins', 'mcp')}><span><Cable /></span><p><b>前往连接器管理</b><small>{mcp.items.length} 个连接 · CLI / MCP / OpenAPI</small></p><ChevronRight /></button></Card></SettingsScaffold></WorkspacePage>
}

export function AboutSettingsPage({ sidebarCollapsed, onSidebar }: { sidebarCollapsed: boolean; onSidebar: () => void }) {
  return <WorkspacePage title="关于" subtitle="Fox Desktop" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}><SettingsScaffold kicker="应用信息" title="关于" description="查看 Fox 版本、技术栈与许可证。"><Card className="fox-settings-section fox-about-product"><img src="/mascot/fox/status/fox_sayhi.png" alt="" /><div><h2>Fox</h2><p>轻量、高效、可连接知识库服务的桌面专家工作台。</p><Badge className="fox-settings-status-pill" variant="secondary">0.1.0</Badge></div></Card><Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>版本</h2><p>当前使用 Tauri 2、React、shadcn/ui、AI Elements 与 Fox Runtime。</p></div><Info /></div><PreferenceRow title="当前版本" description="开发预览版本"><code>0.1.0</code></PreferenceRow></Card><Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>开源与许可</h2><p>Fox 自身许可将在正式发布前确定；第三方依赖遵循各自许可证。</p></div><Archive /></div><PreferenceRow title="架构文档" description="项目 docs 目录包含三个阶段的设计与实施记录"><Badge className="fox-settings-status-pill is-wide" variant="secondary">本地文档</Badge></PreferenceRow></Card></SettingsScaffold></WorkspacePage>
}

export function SkillsPage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const skills = useSkills()
  const [expanded, setExpanded] = useState<string | null>(null)
  return (
    <WorkspacePage title="技能" subtitle="Fox 指令扩展" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={() => navigate('settings')} actions={<Button variant="outline" size="sm" onClick={() => void skills.refresh()}>{skills.loading && <LoaderCircle className="animate-spin" />}重新扫描</Button>}>
      <div className="fox-settings-layout">
        <main><header><span>受控扩展</span><h1>技能</h1><p>扫描 Fox 数据目录下的 `skills/*/SKILL.md`。技能 只注入指令，不会获得新的工具权限。</p></header>{skills.error && <p className="fox-setting-error">{skills.error}</p>}{!skills.loading && skills.items.length === 0 && <Card className="fox-settings-section fox-skill-empty"><Puzzle /><div><h2>尚未发现 技能</h2><p>在 Fox 数据目录的 skills 子目录中创建一个包含 SKILL.md 的文件夹。</p></div></Card>}<div className="fox-skill-list">{skills.items.map((skill) => <Card key={skill.id} className="fox-settings-section fox-skill-card"><div className="fox-skill-card-head"><span><Puzzle /></span><button className="fox-skill-summary" onClick={() => setExpanded((value) => value === skill.id ? null : skill.id)}><h2>{skill.name}</h2><p>{skill.description || skill.id}</p></button><Badge variant={skill.valid ? 'secondary' : 'destructive'}>{skill.valid ? `v${skill.version}` : '校验失败'}</Badge><Switch checked={skill.enabled} disabled={!skill.valid} onCheckedChange={(enabled) => void skills.setEnabled(skill.id, enabled).then((ok) => ok && toast.success(enabled ? `已启用 ${skill.name}` : `已停用 ${skill.name}`))} /></div><Separator /><div className="fox-skill-meta"><span><Wrench />{skill.requiredTools.length ? skill.requiredTools.join(' · ') : '不要求额外工具'}</span><small title={skill.sourcePath}>{skill.sourcePath}</small></div>{skill.validationError && <p className="fox-setting-error">{skill.validationError}</p>}{expanded === skill.id && <pre className="fox-skill-instructions">{skill.instructions}</pre>}</Card>)}</div></main>
      </div>
    </WorkspacePage>
  )
}

interface McpFormState {
  id?: string
  name: string
  command: string
  argsText: string
  transport: McpServerRecord['transport']
  endpointUrl: string
  definition: string
  environmentText: string
  clearEnvironment: boolean
}

const emptyMcpForm: McpFormState = { name: '', command: '', argsText: '', transport: 'stdio', endpointUrl: '', definition: '', environmentText: '', clearEnvironment: false }

interface HookFormState {
  id?: string
  name: string
  event: LifecycleHookRecord['event']
  matcher: string
  action: LifecycleHookRecord['action']
  reason: string
  priority: number
}

const emptyHookForm: HookFormState = { name: '', event: 'before_tool', matcher: '*', action: 'annotate', reason: '', priority: 100 }

function extensionTransportLabel(transport: McpServerRecord['transport']) {
  if (transport === 'streamable_http') return 'HTTP MCP'
  if (transport === 'openapi') return 'OpenAPI'
  return 'stdio MCP'
}

function mcpStatusLabel(server: McpServerRecord) {
  if (!server.enabled) return '已停用'
  if (server.status === 'connected') return '可用'
  if (server.status === 'unavailable') return '不可用'
  return '未测试'
}

function parseEnvironment(value: string) {
  const environment: Record<string, string> = {}
  for (const rawLine of value.split(/\r?\n/)) {
    const line = rawLine.trim()
    if (!line) continue
    const separator = line.indexOf('=')
    if (separator < 1) throw new Error(`环境变量格式无效：${line}`)
    environment[line.slice(0, separator).trim()] = line.slice(separator + 1)
  }
  return environment
}

export function McpPage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const resource = useMcpServers()
  const hooks = useLifecycleHooks()
  const [form, setForm] = useState<McpFormState | null>(null)
  const [hookForm, setHookForm] = useState<HookFormState>({ ...emptyHookForm })
  const [expanded, setExpanded] = useState<string | null>(null)
  const [formError, setFormError] = useState<string | null>(null)
  const openEdit = (server: McpServerRecord) => setForm({ id: server.id, name: server.name, command: server.command, argsText: server.args.join('\n'), transport: server.transport, endpointUrl: server.endpointUrl ?? '', definition: server.definition ?? '', environmentText: '', clearEnvironment: false })
  const save = async () => {
    if (!form) return
    try {
      const environment = parseEnvironment(form.environmentText)
      const input: SaveMcpServerInput = { id: form.id, name: form.name.trim(), command: form.command.trim(), args: form.argsText.split(/\r?\n/).map((value) => value.trim()).filter(Boolean), transport: form.transport, endpointUrl: form.endpointUrl.trim() || undefined, definition: form.definition.trim() || undefined, clearEnvironment: form.clearEnvironment }
      if (Object.keys(environment).length) input.environment = environment
      const saved = await resource.save(input)
      if (!saved) return
      setForm(null); setFormError(null); toast.success(form.id ? 'MCP Server 已更新' : 'MCP Server 已添加')
    } catch (cause) { setFormError(cause instanceof Error ? cause.message : String(cause)) }
  }
  const saveHook = async () => {
    const saved = await hooks.save({ ...hookForm, name: hookForm.name.trim(), matcher: hookForm.matcher.trim(), reason: hookForm.reason.trim(), enabled: true })
    if (!saved) return
    setHookForm({ ...emptyHookForm })
    toast.success(hookForm.id ? '生命周期 Hook 已更新' : '生命周期 Hook 已添加')
  }
  return (
    <WorkspacePage title="连接器" subtitle="CLI、MCP、OpenAPI 与策略" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={() => navigate('settings')} actions={<Button size="sm" onClick={() => { setForm({ ...emptyMcpForm }); setFormError(null) }}><Plus />添加连接器</Button>}>
      <div className="fox-settings-layout">
        <main><header><span>统一扩展协议</span><h1>连接器</h1><p>持久连接 stdio / Streamable HTTP MCP，或把 OpenAPI 3.x operation 动态注册为工具；所有调用继续经过审批、声明式 Hook 与审计。</p></header>
          {resource.error && <p className="fox-setting-error">{resource.error}</p>}
          {!resource.loading && resource.items.length === 0 && <Card className="fox-settings-section fox-skill-empty"><Cable /><div><h2>尚未添加连接器</h2><p>添加本机 MCP、HTTP MCP 或 OpenAPI 定义后，先运行健康测试，再由 Fox Runtime 按需调用。</p></div></Card>}
          <div className="fox-mcp-list">{resource.items.map((server) => { const test = resource.tests[server.id]; const isExpanded = expanded === server.id; const target = server.transport === 'stdio' ? `${server.command}${server.args.length ? ` · ${server.args.join(' ')}` : ''}` : server.endpointUrl ?? ''; return <Card key={server.id} className="fox-settings-section fox-mcp-card"><div className="fox-mcp-card-head"><span><Cable /></span><button className="fox-skill-summary" onClick={() => setExpanded(isExpanded ? null : server.id)}><h2>{server.name}</h2><p title={target}>{server.id === 'fox-office' ? '内置 CLI' : extensionTransportLabel(server.transport)} · {target}</p></button><Badge variant={server.status === 'unavailable' ? 'destructive' : 'secondary'}>{mcpStatusLabel(server)}</Badge><Switch checked={server.enabled} disabled={resource.busyId === server.id} onCheckedChange={(enabled) => void resource.setEnabled(server.id, enabled)} /></div><Separator /><div className="fox-mcp-meta"><span><Zap />{test ? `${test.toolCount} 个工具 · ${test.latencyMs} ms` : server.toolCount != null ? `${server.toolCount} 个工具${server.lastLatencyMs != null ? ` · ${server.lastLatencyMs} ms` : ''}` : '等待健康测试'}</span><span>{server.credentialConfigured ? (server.transport === 'stdio' ? '环境变量已安全保存' : '请求 Header 已安全保存') : '未保存凭证'}</span><div><Button variant="ghost" size="sm" disabled={!server.enabled || resource.busyId === server.id} onClick={() => void resource.test(server.id).then((result) => result && toast.success(`健康检查通过，发现 ${result.toolCount} 个工具`))}>{resource.busyId === server.id ? <LoaderCircle className="animate-spin" /> : <Zap />}健康检查</Button><Button variant="ghost" size="icon-sm" title="编辑" disabled={server.id === 'fox-office'} onClick={() => openEdit(server)}><Pencil /></Button><Button variant="ghost" size="icon-sm" title="删除" disabled={server.id === 'fox-office'} onClick={() => { if (window.confirm(`删除连接器“${server.name}”？已保存的凭证也会被清除。`)) void resource.remove(server.id).then((ok) => ok && toast.success('连接器已删除')) }}><Trash2 /></Button><button className="fox-mcp-expand" aria-label={isExpanded ? '收起工具' : '展开工具'} onClick={() => setExpanded(isExpanded ? null : server.id)}><ChevronDown className={isExpanded ? 'is-open' : ''} /></button></div></div>{server.lastError && <p className="fox-setting-error">{server.lastError}</p>}{isExpanded && <div className="fox-mcp-tools">{test?.tools.length ? test.tools.map((tool) => <div key={tool.name}><b>{tool.name}</b><p>{tool.description || '此工具没有提供说明。'}</p></div>) : <p>点击“健康检查”读取并校验工具目录。Fox 每个连接器最多接收 200 个工具。</p>}</div>}</Card> })}</div>
          <header><span>受限策略生命周期</span><h1>声明式 Hooks</h1><p>Hook 不执行脚本，只能按工具名匹配并阻止、要求审批或向模型附加说明；每次命中都会写入审计记录。</p></header>
          {hooks.error && <p className="fox-setting-error">{hooks.error}</p>}
          <Card className="fox-settings-section fox-service-form"><div className="fox-settings-section-head"><div><h2>{hookForm.id ? '编辑 Hook' : '添加 Hook'}</h2><p>匹配器支持精确名称、逗号分组，以及前后缀通配符，例如 call_*。</p></div><ShieldCheck /></div><div className="fox-settings-form-grid"><label>名称<Input value={hookForm.name} onChange={(event) => setHookForm({ ...hookForm, name: event.target.value })} placeholder="例如 高风险工具审批" /></label><label>事件<Select value={hookForm.event} onValueChange={(event: LifecycleHookRecord['event']) => setHookForm({ ...hookForm, event, action: event === 'before_tool' ? hookForm.action : 'annotate' })}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="before_tool">工具调用前</SelectItem><SelectItem value="after_tool">工具调用后</SelectItem><SelectItem value="before_run">Run 开始前</SelectItem><SelectItem value="after_run">Run 结束后</SelectItem></SelectContent></Select></label><label>匹配器<Input value={hookForm.matcher} onChange={(event) => setHookForm({ ...hookForm, matcher: event.target.value })} placeholder="run_command, call_*" /></label><label>动作<Select value={hookForm.action} onValueChange={(action: LifecycleHookRecord['action']) => setHookForm({ ...hookForm, action })}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent>{hookForm.event === 'before_tool' && <SelectItem value="block">阻止</SelectItem>}{hookForm.event === 'before_tool' && <SelectItem value="require_approval">要求审批</SelectItem>}<SelectItem value="annotate">附加说明</SelectItem></SelectContent></Select></label><label>原因 / 说明<Textarea value={hookForm.reason} onChange={(event) => setHookForm({ ...hookForm, reason: event.target.value })} placeholder="会展示在审批或模型上下文中" /></label></div><div className="fox-setting-actions">{hookForm.id && <Button variant="outline" onClick={() => setHookForm({ ...emptyHookForm })}>取消编辑</Button>}<Button disabled={!hookForm.name.trim() || !hookForm.matcher.trim() || hooks.busyId != null} onClick={() => void saveHook()}>{hooks.busyId && <LoaderCircle className="animate-spin" />}保存 Hook</Button></div></Card>
          <div className="fox-mcp-list">{hooks.items.map((hook) => <Card key={hook.id} className="fox-settings-section fox-mcp-card"><div className="fox-mcp-card-head"><span><Shield /></span><button className="fox-skill-summary" onClick={() => setHookForm({ id: hook.id, name: hook.name, event: hook.event, matcher: hook.matcher, action: hook.action, reason: hook.reason, priority: hook.priority })}><h2>{hook.name}</h2><p>{hook.event} · {hook.matcher} · {hook.action}</p></button><Badge variant={hook.action === 'block' ? 'destructive' : 'secondary'}>{hook.action}</Badge><Switch checked={hook.enabled} disabled={hooks.busyId === hook.id} onCheckedChange={(enabled) => void hooks.setEnabled(hook.id, enabled)} /></div><Separator /><div className="fox-mcp-meta"><span>{hook.reason || '未填写说明'}</span><div><Button variant="ghost" size="icon-sm" title="编辑" onClick={() => setHookForm({ id: hook.id, name: hook.name, event: hook.event, matcher: hook.matcher, action: hook.action, reason: hook.reason, priority: hook.priority })}><Pencil /></Button><Button variant="ghost" size="icon-sm" title="删除" onClick={() => { if (window.confirm(`删除 Hook“${hook.name}”？`)) void hooks.remove(hook.id).then((ok) => ok && toast.success('Hook 已删除')) }}><Trash2 /></Button></div></div></Card>)}</div>
        </main>
      </div>
      <Dialog open={Boolean(form)} onOpenChange={(open) => { if (!open) setForm(null) }}><DialogContent className="fox-mcp-dialog"><DialogHeader><DialogTitle>{form?.id ? '编辑连接器' : '添加连接器'}</DialogTitle><DialogDescription>远程凭证只存入系统凭证库；HTTP MCP 和 OpenAPI 禁止自动重定向，超时 15 秒，响应上限 10 MB。</DialogDescription></DialogHeader>{form && <div className="fox-mcp-form"><label>名称<Input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} placeholder="例如 Filesystem 或 Pet API" /></label><label>扩展类型<Select value={form.transport} onValueChange={(transport: McpServerRecord['transport']) => setForm({ ...form, transport })}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="stdio">stdio MCP</SelectItem><SelectItem value="streamable_http">Streamable HTTP MCP</SelectItem><SelectItem value="openapi">OpenAPI 3.x Connector</SelectItem></SelectContent></Select></label>{form.transport === 'stdio' ? <><label>命令<Input value={form.command} onChange={(event) => setForm({ ...form, command: event.target.value })} placeholder="例如 npx.cmd 或 MCP Server 可执行文件路径" /></label><label>参数（每行一项）<Textarea value={form.argsText} onChange={(event) => setForm({ ...form, argsText: event.target.value })} placeholder={'-y\n@modelcontextprotocol/server-filesystem\nD:\\projects'} /></label></> : <label>端点 URL{form.transport === 'openapi' ? '（可选，覆盖 servers[0].url）' : ''}<Input value={form.endpointUrl} onChange={(event) => setForm({ ...form, endpointUrl: event.target.value })} placeholder={form.transport === 'openapi' ? '可留空使用定义中的 servers[0].url' : 'https://mcp.example.com/mcp'} /></label>}{form.transport === 'openapi' && <label>OpenAPI JSON / YAML<Textarea value={form.definition} onChange={(event) => setForm({ ...form, definition: event.target.value })} placeholder={'openapi: 3.0.3\ninfo: ...\npaths: ...'} /></label>}<label>{form.transport === 'stdio' ? '环境变量' : '请求 Header'}（每行 KEY=VALUE）<Textarea value={form.environmentText} onChange={(event) => setForm({ ...form, environmentText: event.target.value, clearEnvironment: false })} placeholder={form.id ? '已保存的值不会回显；留空保持不变' : form.transport === 'stdio' ? 'API_KEY=...' : 'Authorization=Bearer ...'} /></label>{form.id && <div className="fox-setting-row"><span><b>清除已保存凭证</b><small>保存时同时从系统凭证库删除</small></span><Switch checked={form.clearEnvironment} onCheckedChange={(clearEnvironment) => setForm({ ...form, clearEnvironment, environmentText: clearEnvironment ? '' : form.environmentText })} /></div>}{formError && <p className="fox-setting-error">{formError}</p>}</div>}<DialogFooter><Button variant="outline" onClick={() => setForm(null)}>取消</Button><Button disabled={!form?.name.trim() || (form.transport === 'stdio' && !form.command.trim()) || (form.transport === 'streamable_http' && !form.endpointUrl.trim()) || (form.transport === 'openapi' && !form.definition.trim()) || resource.busyId != null} onClick={() => void save()}>{resource.busyId && <LoaderCircle className="animate-spin" />}保存</Button></DialogFooter></DialogContent></Dialog>
    </WorkspacePage>
  )
}

type AppDiagnosticCheck = {
  id: string
  label: string
  status: 'ok' | 'warning' | 'error' | 'offline'
  summary: string
  route?: Parameters<NavigateWorkspace>[0]
  actionLabel?: string
}

function diagnosticStatusLabel(status: AppDiagnosticCheck['status']) {
  return ({ ok: '正常', warning: '需注意', error: '异常', offline: '离线' })[status]
}

export function MaintenancePage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const [diagnostics, setDiagnostics] = useState<Awaited<ReturnType<typeof desktopClient.runtimeDiagnostics>> | null>(null)
  const [diagnosticChecks, setDiagnosticChecks] = useState<AppDiagnosticCheck[]>([])
  const [diagnosticsLoading, setDiagnosticsLoading] = useState(false)
  const [previewCache, setPreviewCache] = useState<KnowledgePreviewCacheStatistics | null>(null)
  const [restorePath, setRestorePath] = useState('')
  const [retentionDays, setRetentionDays] = useState('30')
  const [busy, setBusy] = useState<string | null>(null)
  const [lastPath, setLastPath] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const run = async (key: string, operation: () => Promise<{ message: string; path: string | null; restartRequired: boolean }>) => {
    setBusy(key); setError(null)
    try { const result = await operation(); setLastPath(result.path); toast.success(result.message); if (result.restartRequired) toast('请关闭并重新启动 Fox 以应用恢复') }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBusy(null) }
  }
  const refreshPreviewCache = async () => {
    try { setPreviewCache(await desktopClient.knowledgePreviewCacheStatistics()) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }
  const clearPreviewCache = async () => {
    setBusy('preview-cache'); setError(null)
    try {
      const result = await desktopClient.clearKnowledgePreviewCache()
      toast.success(`${result.message}，释放 ${formatStorageSize(result.bytes)}`)
      await refreshPreviewCache()
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBusy(null) }
  }
  const updatePreviewCacheLimit = async (value: string) => {
    const limitBytes = Number(value)
    if (!Number.isFinite(limitBytes)) return
    setBusy('preview-cache-limit'); setError(null)
    try {
      setPreviewCache(await desktopClient.setKnowledgePreviewCacheLimit(limitBytes))
      toast.success(`预览缓存上限已调整为 ${formatStorageSize(limitBytes)}`)
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBusy(null) }
  }
  const runFullDiagnostics = async () => {
    setDiagnosticsLoading(true)
    const checks: AppDiagnosticCheck[] = []
    const [runtimeResult, providersResult, storageResult, basesResult, jobsResult, modelsResult, vectorResult, mcpResult, pluginsResult, projectsResult] = await Promise.allSettled([
      desktopClient.runtimeDiagnostics(),
      desktopClient.listModelProviders(),
      defaultLocalKnowledgeGateway.getStorageStatus(),
      defaultLocalKnowledgeGateway.listKnowledgeBases(),
      defaultLocalKnowledgeGateway.listJobs(),
      defaultLocalKnowledgeGateway.listEmbeddingModels(),
      defaultLocalKnowledgeGateway.getVectorBackendHealth(),
      desktopClient.listMcpServers(),
      defaultPluginGateway.installationsList(),
      desktopClient.listManagedProjects(),
    ])
    if (runtimeResult.status === 'fulfilled') {
      setDiagnostics(runtimeResult.value)
      checks.push({ id: 'runtime', label: 'Runtime Host', status: runtimeResult.value.state === 'ready' ? 'ok' : 'warning', summary: `${runtimeResult.value.runtime} · 协议 v${runtimeResult.value.protocolVersion} · ${runtimeResult.value.state}` })
    } else checks.push({ id: 'runtime', label: 'Runtime Host', status: 'error', summary: '无法读取 Runtime 诊断信息' })
    if (providersResult.status === 'fulfilled') {
      const enabled = providersResult.value.filter((item) => item.enabled)
      const healthy = enabled.filter((item) => item.lastStatus === 'connected')
      checks.push({ id: 'models', label: '模型连接', status: enabled.length === 0 ? 'warning' : healthy.length === enabled.length ? 'ok' : 'warning', summary: enabled.length === 0 ? '尚未启用模型供应商' : `${healthy.length} / ${enabled.length} 个已启用供应商连接正常`, route: 'settings-models', actionLabel: '查看模型' })
    } else checks.push({ id: 'models', label: '模型连接', status: 'error', summary: '模型供应商状态读取失败', route: 'settings-models', actionLabel: '检查配置' })
    if (storageResult.status === 'fulfilled') {
      checks.push({ id: 'knowledge-storage', label: '本地知识库', status: storageResult.value.writable && !storageResult.value.restartRequired ? 'ok' : 'warning', summary: `数据库 v${storageResult.value.schemaVersion} · ${storageResult.value.writable ? '目录可写' : '目录不可写'}${storageResult.value.restartRequired ? ' · 等待重启完成迁移' : ''}`, route: 'local-knowledge-home', actionLabel: '查看存储' })
    } else checks.push({ id: 'knowledge-storage', label: '本地知识库', status: 'error', summary: '无法读取数据库与目录权限', route: 'local-knowledge-home', actionLabel: '查看存储' })
    if (basesResult.status === 'fulfilled' && jobsResult.status === 'fulfilled') {
      const activeJobs = jobsResult.value.filter((item) => ['queued', 'running', 'paused'].includes(item.status))
      const failedJobs = jobsResult.value.filter((item) => item.status === 'failed')
      checks.push({ id: 'knowledge-jobs', label: '知识任务队列', status: failedJobs.length ? 'warning' : 'ok', summary: `${basesResult.value.length} 个知识库 · ${activeJobs.length} 个活动任务 · ${failedJobs.length} 个失败任务`, route: 'local-knowledge-jobs', actionLabel: '查看任务' })
    } else checks.push({ id: 'knowledge-jobs', label: '知识任务队列', status: 'offline', summary: '知识任务队列不可用', route: 'local-knowledge-jobs', actionLabel: '查看任务' })
    if (modelsResult.status === 'fulfilled') {
      const readyModels = modelsResult.value.filter((item) => item.status === 'ready' && item.integrityStatus === 'verified' && item.loadReady)
      const damagedModels = modelsResult.value.filter((item) => item.status === 'error' || item.integrityStatus === 'failed')
      checks.push({ id: 'embedding', label: '向量模型', status: damagedModels.length ? 'error' : readyModels.length ? 'ok' : 'warning', summary: `${readyModels.length} 个可加载模型${damagedModels.length ? ` · ${damagedModels.length} 个完整性异常` : ''}`, route: 'local-knowledge-models', actionLabel: readyModels.length ? '管理模型' : '安装模型' })
    } else checks.push({ id: 'embedding', label: '向量模型', status: 'error', summary: '模型清单或 ONNX 运行状态读取失败', route: 'local-knowledge-models', actionLabel: '检查模型' })
    if (vectorResult.status === 'fulfilled') {
      checks.push({ id: 'zvec', label: 'Zvec 向量后端', status: vectorResult.value.readWriteVerified ? 'ok' : vectorResult.value.fallbackActive ? 'warning' : 'error', summary: `${vectorResult.value.backend} · ${vectorResult.value.message}${vectorResult.value.fallbackActive ? ' · 已启用降级存储' : ''}`, route: 'local-knowledge-models', actionLabel: '查看向量模型' })
    } else checks.push({ id: 'zvec', label: 'Zvec 向量后端', status: 'error', summary: '读写自检未完成', route: 'local-knowledge-models', actionLabel: '重新检查' })
    if (mcpResult.status === 'fulfilled') {
      const enabled = mcpResult.value.filter((item) => item.enabled)
      const failed = enabled.filter((item) => item.status === 'unavailable' || Boolean(item.lastError))
      checks.push({ id: 'mcp', label: '连接器', status: failed.length ? 'warning' : 'ok', summary: `${enabled.length} 个已启用 · ${failed.length} 个最近失败`, route: 'settings-extensions', actionLabel: '查看扩展' })
    } else checks.push({ id: 'mcp', label: '连接器', status: 'error', summary: '扩展连接状态读取失败', route: 'settings-extensions', actionLabel: '检查扩展' })
    if (pluginsResult.status === 'fulfilled') {
      const failed = pluginsResult.value.items.filter((item) => !item.compatible || Boolean(item.lastError))
      checks.push({ id: 'plugins', label: '插件', status: failed.length ? 'warning' : 'ok', summary: `${pluginsResult.value.total} 个已安装 · ${failed.length} 个兼容性或运行异常`, route: 'plugins', actionLabel: '查看插件' })
    } else checks.push({ id: 'plugins', label: '插件', status: 'error', summary: '已安装插件状态读取失败', route: 'plugins', actionLabel: '查看插件' })
    if (projectsResult.status === 'fulfilled') {
      const missing = projectsResult.value.filter((item) => item.archivedAt === null && !item.pathExists)
      checks.push({ id: 'system-paths', label: '系统与项目路径', status: missing.length ? 'warning' : 'ok', summary: `${projectsResult.value.length} 个项目 · ${missing.length} 个路径失效`, route: 'settings-projects', actionLabel: '管理项目' })
    } else checks.push({ id: 'system-paths', label: '系统与项目路径', status: 'error', summary: '系统路径能力读取失败', route: 'settings-projects', actionLabel: '检查项目' })
    setDiagnosticChecks(checks)
    setDiagnosticsLoading(false)
  }
  const copyDiagnosticSummary = async () => {
    const lines = ['Fox 脱敏诊断摘要', `生成时间：${new Date().toLocaleString('zh-CN')}`, ...diagnosticChecks.map((item) => `[${diagnosticStatusLabel(item.status)}] ${item.label}：${item.summary}`)]
    try {
      await navigator.clipboard.writeText(lines.join('\n'))
      toast.success('脱敏诊断摘要已复制')
    } catch {
      toast.error('无法访问剪贴板')
    }
  }
  useEffect(() => {
    void runFullDiagnostics()
    void refreshPreviewCache()
  }, [])
  const cacheRatio = previewCache?.limitBytes ? Math.min(1, previewCache.totalBytes / previewCache.limitBytes) : 0
  const clearableBytes = Math.max(0, (previewCache?.totalBytes ?? 0) - (previewCache?.activeBytes ?? 0))
  return <WorkspacePage title="数据与诊断" subtitle="备份、恢复与维护" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={() => navigate('settings')}>
    <div className="fox-settings-layout">
      <main><header><span>系统维护</span><h1>数据与诊断</h1><p>管理 Fox 本地数据。备份不包含 API Key、Token 或其他系统凭证库内容。</p></header>
        {error && <p className="fox-setting-error">{error}</p>}
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>统一诊断</h2><p>检查 Runtime、模型、知识库、向量、扩展与系统路径；摘要不包含凭证、正文或文件内容。</p></div><Badge variant={diagnosticChecks.some((item) => item.status === 'error') ? 'destructive' : 'secondary'}>{diagnosticsLoading ? '检测中' : diagnosticChecks.some((item) => item.status === 'error') ? '发现异常' : '检查完成'}</Badge></div><div className="fox-diagnostic-checks" aria-live="polite">{diagnosticsLoading && diagnosticChecks.length === 0 ? <div className="fox-diagnostic-loading"><LoaderCircle className="animate-spin" />正在逐项运行自检</div> : diagnosticChecks.map((item) => <div key={item.id} className={`is-${item.status}`}><span>{item.status === 'ok' ? <Check /> : item.status === 'warning' ? <FileWarning /> : item.status === 'offline' ? <Minus /> : <FileWarning />}</span><p><b>{item.label}</b><small>{item.summary}</small></p><Badge className="fox-diagnostic-status" variant={item.status === 'error' ? 'destructive' : 'outline'}>{diagnosticStatusLabel(item.status)}</Badge>{item.route && <Button variant="ghost" size="sm" onClick={() => navigate(item.route!)}>{item.actionLabel ?? '查看'}<ChevronRight /></Button>}</div>)}</div><div className="fox-maintenance-stats"><span><b>{diagnostics?.runtime ?? '--'}</b><small>Runtime</small></span><span><b>v{diagnostics?.protocolVersion ?? '--'}</b><small>协议</small></span><span><b>{diagnostics?.sessionFileCount ?? '--'}</b><small>Session</small></span><span><b>{diagnostics?.recoveryAttempts ?? 0}</b><small>恢复尝试</small></span></div><div className="fox-setting-actions"><Button variant="outline" size="sm" disabled={diagnosticsLoading || busy != null} onClick={() => void runFullDiagnostics()}><RotateCcw className={diagnosticsLoading ? 'animate-spin' : undefined} />重新检查</Button><Button variant="outline" size="sm" disabled={!diagnosticChecks.length} onClick={() => void copyDiagnosticSummary()}><Copy />复制脱敏摘要</Button><Button size="sm" disabled={busy != null} onClick={() => void run('diagnostics', desktopClient.exportDiagnostics)}>{busy === 'diagnostics' ? <LoaderCircle className="animate-spin" /> : <Download />}导出诊断包</Button></div></Card>
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>预览缓存</h2><p>Fox 缓存知识库原文件以加快再次预览，达到上限后会自动按最近使用时间清理。</p></div><Badge variant="secondary">{previewCache ? `${Math.round(cacheRatio * 100)}%` : '读取中'}</Badge></div><div className="fox-preview-cache-meter" aria-label="预览缓存占用"><i style={{ width: `${cacheRatio * 100}%` }} /></div><div className="fox-maintenance-stats"><span><b>{previewCache ? formatStorageSize(previewCache.totalBytes) : '--'}</b><small>已使用</small></span><span><b>{previewCache ? formatStorageSize(clearableBytes) : '--'}</b><small>可清理</small></span><span><b>{previewCache?.activeFiles ?? '--'}</b><small>正在使用</small></span><span><b>{previewCache ? formatStorageSize(previewCache.limitBytes) : '--'}</b><small>缓存上限</small></span></div><PreferenceRow title="最大缓存空间" description="调小后会立即淘汰未使用的旧文件，正在打开的预览会保留"><Select value={previewCache ? String(previewCache.limitBytes) : undefined} disabled={busy != null || !previewCache} onValueChange={(value) => void updatePreviewCacheLimit(value)}><SelectTrigger className="fox-settings-select"><SelectValue placeholder="选择上限" /></SelectTrigger><SelectContent><SelectItem value={String(250 * 1024 * 1024)}>250 MB</SelectItem><SelectItem value={String(500 * 1024 * 1024)}>500 MB</SelectItem><SelectItem value={String(1024 * 1024 * 1024)}>1 GB</SelectItem><SelectItem value={String(2 * 1024 * 1024 * 1024)}>2 GB</SelectItem><SelectItem value={String(5 * 1024 * 1024 * 1024)}>5 GB</SelectItem></SelectContent></Select></PreferenceRow><div className="fox-setting-actions"><Button variant="outline" size="sm" disabled={busy != null} onClick={() => void refreshPreviewCache()}><RotateCcw />刷新占用</Button><Button variant="outline" size="sm" disabled={busy != null || !previewCache || clearableBytes === 0} onClick={() => { if (window.confirm('清理所有未使用的知识库预览缓存？正在打开的文件会保留。')) void clearPreviewCache() }}>{busy === 'preview-cache' ? <LoaderCircle className="animate-spin" /> : <Trash2 />}清理未使用缓存</Button></div></Card>
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>备份 Fox</h2><p>包含 SQLite、附件、技能与 Runtime Session。Session 仅尽力跨版本恢复。</p></div><Archive /></div><div className="fox-setting-actions"><Button disabled={busy != null} onClick={() => void run('backup', desktopClient.createBackup)}>{busy === 'backup' ? <LoaderCircle className="animate-spin" /> : <HardDrive />}创建备份</Button></div></Card>
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>恢复备份</h2><p>先验证清单、哈希、路径与数据库完整性，下次启动 Fox 时应用。</p></div><FileWarning /></div><label className="fox-maintenance-field">备份文件路径<Input value={restorePath} onChange={(event) => setRestorePath(event.target.value)} placeholder="D:\\Backups\\fox-backup-....foxbackup" /></label><div className="fox-setting-actions"><Button variant="outline" disabled={busy != null || !restorePath.trim()} onClick={() => { if (window.confirm('恢复会在下次启动时替换当前 Fox 核心数据，是否继续？')) void run('restore', () => desktopClient.restoreBackup(restorePath.trim())) }}>{busy === 'restore' ? <LoaderCircle className="animate-spin" /> : <RotateCcw />}验证并安排恢复</Button></div></Card>
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>清理本地数据</h2><p>清理超过保留期的孤立 Runtime Session、孤立附件和失败记录，不触碰项目目录中的已导出文件。</p></div><Trash2 /></div><label className="fox-maintenance-field">孤立 Session 保留天数<Input inputMode="numeric" value={retentionDays} onChange={(event) => setRetentionDays(event.target.value)} /></label><div className="fox-setting-actions"><Button variant="outline" disabled={busy != null} onClick={() => { if (window.confirm('开始清理 Fox 内部的孤立文件？')) void run('cleanup', () => desktopClient.cleanupData(Math.max(1, Number(retentionDays) || 30))) }}>{busy === 'cleanup' ? <LoaderCircle className="animate-spin" /> : <Trash2 />}开始清理</Button></div></Card>
        {lastPath && <Card className="fox-maintenance-result"><Check /><div><b>操作完成</b><p title={lastPath}>{lastPath}</p></div></Card>}
      </main>
    </div>
  </WorkspacePage>
}

function formatStorageSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`
}

export function ServicePage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const yuxi = useYuxiService()
  const [name, setName] = useState('本地知识库')
  const [baseUrl, setBaseUrl] = useState('http://127.0.0.1:5050')
  const [token, setToken] = useState('')

  const returnFromService = () => {
    const returnView = window.sessionStorage.getItem(YUXI_SERVICE_RETURN_KEY)
    window.sessionStorage.removeItem(YUXI_SERVICE_RETURN_KEY)
    navigate(resolveYuxiServiceReturn(returnView))
  }

  useEffect(() => {
    if (!yuxi.service) return
    setName(yuxi.service.name)
    setBaseUrl(yuxi.service.baseUrl)
  }, [yuxi.service])

  const testConnection = async () => {
    const result = await yuxi.test(baseUrl, token || undefined)
    if (result) toast.success(result.authenticated ? '连接成功，访问凭证已验证' : `知识库服务连接成功 · ${result.version ?? '版本未知'}`)
  }

  const saveConnection = async () => {
    const saved = await yuxi.save({ name, baseUrl, accessToken: token || undefined })
    if (!saved) return
    await yuxi.test(saved.baseUrl)
    setToken('')
    toast.success('知识库服务配置已保存')
    returnFromService()
  }

  const result = yuxi.testResult
  return (
    <WorkspacePage title="连接知识库" subtitle="服务配置" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={returnFromService}>
      <div className="fox-settings-layout"><main><header><span>服务连接</span><h1>知识库服务</h1><p>连接本机、局域网或远程知识库服务，为 Fox 提供远程专家、知识库和知识图谱能力。</p></header><Card className="fox-settings-section fox-service-form"><div className="fox-settings-section-head"><div><h2>连接配置</h2><p>Fox 首版仅保存一个知识库服务地址。</p></div><Server /></div><div className="fox-settings-form-grid"><label>服务名称<Input value={name} onChange={(event) => setName(event.target.value)} /></label><label>API 地址 <b>*</b><Input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="http://127.0.0.1:5050" /></label><small className="fox-field-help">远程示例：https://knowledge.example.com，Fox 会检测 /api/system/health。</small><label>访问令牌<Input type="password" value={token} onChange={(event) => setToken(event.target.value)} placeholder={yuxi.service?.credentialConfigured ? '已保存，留空则保持不变' : '可选：API Key 或登录 Token'} /></label></div>{result ? <Card className="fox-connection-result"><span><Server /></span><p><b>{result.authenticated ? '凭证已验证' : '服务可达'}</b><small>{result.version ?? '未知版本'} · {result.latencyMs} ms · {connectionTypeLabel(result.connectionType)}</small></p><Badge variant="secondary">{result.authenticated ? '已鉴权' : '可用'}</Badge></Card> : yuxi.error ? <Card className="fox-connection-result is-error"><span><Server /></span><p><b>连接测试失败</b><small>{yuxi.error}</small></p><Badge variant="destructive">不可用</Badge></Card> : null}<div className="fox-setting-actions"><Button variant="outline" disabled={yuxi.testing || !baseUrl.trim()} onClick={() => void testConnection()}>{yuxi.testing && <LoaderCircle className="animate-spin" />}测试连接</Button><Button disabled={yuxi.saving || !name.trim() || !baseUrl.trim()} onClick={() => void saveConnection()}>{yuxi.saving && <LoaderCircle className="animate-spin" />}保存配置</Button></div><small className="fox-settings-security-note"><Shield />Token 使用系统凭证库保存，不写入 Fox 数据库。</small></Card></main></div>
    </WorkspacePage>
  )
}

export function ModelServicePage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const model = useModelService()
  const [name, setName] = useState('Fox 模型服务')
  const [baseUrl, setBaseUrl] = useState('http://127.0.0.1:11434/v1')
  const [modelId, setModelId] = useState('')
  const [apiType, setApiType] = useState<'openai-completions' | 'anthropic-messages'>('openai-completions')
  const [apiKey, setApiKey] = useState('')
  const [contextWindow, setContextWindow] = useState('128000')
  const [maxOutputTokens, setMaxOutputTokens] = useState('8192')
  const [supportsImageInput, setSupportsImageInput] = useState(false)

  useEffect(() => {
    if (!model.service) return
    setName(model.service.name)
    setBaseUrl(model.service.baseUrl)
    setModelId(model.service.modelId)
    setApiType(model.service.apiType)
    setContextWindow(String(model.service.contextWindow))
    setMaxOutputTokens(String(model.service.maxOutputTokens))
    setSupportsImageInput(model.service.supportsImageInput)
  }, [model.service])

  const testConnection = async () => {
    const result = await model.test(baseUrl, apiKey || undefined, apiType, modelId)
    if (!result) return
    if (!modelId && result.models[0]) setModelId(result.models[0])
    toast.success(`连接成功，发现 ${result.models.length} 个模型`)
  }

  const saveConnection = async () => {
    const saved = await model.save({ name, baseUrl, modelId, apiType, contextWindow: Number(contextWindow), maxOutputTokens: Number(maxOutputTokens), supportsImageInput, apiKey: apiKey || undefined })
    if (!saved) return
    await model.test(saved.baseUrl)
    setApiKey('')
    toast.success('模型服务配置已保存')
    navigate('settings-models')
  }

  const result = model.testResult
  return (
    <WorkspacePage title="连接模型服务" subtitle="Fox Runtime" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={() => navigate('settings-models')}>
      <div className="fox-settings-layout"><main><header><span>模型连接</span><h1>模型服务</h1><p>配置 Fox 原生 Agent 使用的 OpenAI-compatible 或 Anthropic Messages 模型服务。</p></header><Card className="fox-settings-section fox-service-form"><div className="fox-settings-section-head"><div><h2>运行配置</h2><p>模型服务与知识库服务相互独立。</p></div><Bot /></div><div className="fox-settings-form-grid"><label>服务名称<Input value={name} onChange={(event) => setName(event.target.value)} /></label><label>API 协议 <b>*</b><Select value={apiType} onValueChange={(value) => setApiType(value as typeof apiType)}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="openai-completions">OpenAI-compatible</SelectItem><SelectItem value="anthropic-messages">Anthropic Messages</SelectItem></SelectContent></Select></label><label>API 地址 <b>*</b><Input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder={apiType === 'anthropic-messages' ? 'https://api.minimaxi.com/anthropic' : 'http://127.0.0.1:11434/v1'} /></label><small className="fox-field-help">{apiType === 'anthropic-messages' ? '填写 Anthropic API 根路径；MiniMax 国内订阅可使用 https://api.minimaxi.com/anthropic。' : '填写 OpenAI-compatible API 根路径，Fox 会检测 /models。'}</small><label>模型 ID <b>*</b><Input value={modelId} onChange={(event) => setModelId(event.target.value)} placeholder={apiType === 'anthropic-messages' ? '例如 MiniMax-M3' : '例如 qwen3:8b 或 deepseek-chat'} /></label><div className="fox-model-limits"><label>上下文长度<Input inputMode="numeric" value={contextWindow} onChange={(event) => setContextWindow(event.target.value)} /></label><label>最大输出<Input inputMode="numeric" value={maxOutputTokens} onChange={(event) => setMaxOutputTokens(event.target.value)} /></label></div><div className="fox-setting-row"><span><b>支持图片输入</b><small>仅在当前模型明确支持视觉理解时开启</small></span><Switch checked={supportsImageInput} onCheckedChange={setSupportsImageInput} /></div><label>API Key<Input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder={model.service?.credentialConfigured ? '已保存，留空则保持不变' : apiType === 'anthropic-messages' ? 'Anthropic Messages 必填' : '本地服务可留空'} /></label></div>{result ? <Card className="fox-connection-result"><span><Bot /></span><p><b>连接测试成功</b><small>{result.models.length} 个模型 · {result.latencyMs} ms · {connectionTypeLabel(result.connectionType)}</small></p><Badge variant="secondary">可用</Badge></Card> : model.error ? <Card className="fox-connection-result is-error"><span><Bot /></span><p><b>连接测试失败</b><small>{model.error}</small></p><Badge variant="destructive">不可用</Badge></Card> : null}<div className="fox-setting-actions"><Button variant="outline" disabled={model.testing || !baseUrl.trim() || !modelId.trim()} onClick={() => void testConnection()}>{model.testing && <LoaderCircle className="animate-spin" />}测试连接</Button><Button disabled={model.saving || !name.trim() || !baseUrl.trim() || !modelId.trim()} onClick={() => void saveConnection()}>{model.saving && <LoaderCircle className="animate-spin" />}保存配置</Button></div><small className="fox-settings-security-note"><Shield />API Key 使用系统凭证库保存，不写入 Fox 数据库。</small></Card></main></div>
    </WorkspacePage>
  )
}

export function LoginPage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const returnFromLogin = () => {
    const returnView = window.sessionStorage.getItem(YUXI_LOGIN_RETURN_KEY)
    window.sessionStorage.removeItem(YUXI_LOGIN_RETURN_KEY)
    navigate(resolveYuxiLoginReturn(returnView))
  }
  const openService = () => {
    window.sessionStorage.setItem(YUXI_SERVICE_RETURN_KEY, 'login')
    navigate('service')
  }
  const login = async () => {
    setBusy(true); setError(null)
    try { const user = await desktopClient.loginYuxi(username, password); window.dispatchEvent(new Event('fox:yuxi-user-changed')); toast.success(`欢迎回来，${user.username}`); returnFromLogin() }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBusy(false) }
  }
  return (
    <WorkspacePage title="知识库登录" subtitle="账户身份验证" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={returnFromLogin}>
      <div className="fox-login-page"><section><img src="/mascot/fox_magic.png" alt="" /><span>欢迎回来</span><h1>知识库登录</h1><p>登录后将同步你有权访问的远程专家与知识库。</p></section><Card><header><span><LogIn /></span><div><h2>使用知识库账户</h2><p>连接到已配置的知识库服务</p></div></header><label>用户名<Input value={username} onChange={(event) => setUsername(event.target.value)} /></label><label>密码<Input type="password" value={password} onChange={(event) => setPassword(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') void login() }} /></label>{error && <p className="fox-setting-error">{error}</p>}<Button size="lg" disabled={busy || !username || !password} onClick={() => void login()}>{busy && <LoaderCircle className="animate-spin" />}登录并进入 Fox</Button><Separator /><Button variant="outline" size="lg" onClick={openService}><ExternalLink />检查服务配置</Button></Card></div>
    </WorkspacePage>
  )
}
