import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'
import { Activity, Archive, BellRing, Bot, Cable, Check, ChevronDown, ChevronRight, Copy, Download, ExternalLink, Eye, EyeOff, FileText, FileWarning, FolderOpen, HardDrive, Info, LoaderCircle, LogIn, MessageSquare, Minus, MoreHorizontal, Pencil, Plus, Puzzle, RotateCcw, Server, Shield, ShieldCheck, Sparkles, Star, Trash2, Volume2, Wrench, Zap } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { cn } from '@/lib/utils'
import { AnimatePresence, motion } from 'motion/react'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { Textarea } from '@/components/ui/textarea'
import { Separator } from '@/components/ui/separator'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { useYuxiService } from './use-yuxi-service'
import { useModelService } from './use-model-service'
import { useYuxiUser } from './use-yuxi-user'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { useSkills } from './use-skills'
import { useMcpServers, type SaveMcpServerInput } from './use-mcp-servers'
import { LifecycleHooksSection } from './lifecycle-hooks-section'
import { SettingsLoadError } from './settings-load-error'
import type { AgentRecord, KnowledgePreviewCacheStatistics, McpServerRecord, ModelProviderRecord, NotificationPreferencesRecord, ObservabilityStatistics, ProjectManagementRecord, ProjectRecord, UsageStatistics } from '@/features/conversations/model/types'
import { Grainient } from '@/components/effects/grainient'
import { BorderBeam } from '@/components/effects/border-beam'
import { UserProfileDialog, useUserProfile } from '@/features/profile/user-profile'
import { resolveYuxiLoginReturn, resolveYuxiServiceReturn, YUXI_LOGIN_RETURN_KEY, YUXI_SERVICE_RETURN_KEY } from '@/features/knowledge/knowledge-navigation'
import { normalizeTextScale, persistTextScale, readTextScale, TEXT_SCALE_MAX, TEXT_SCALE_MIN } from './text-scale'
import { persistColorTheme, readColorTheme, type ColorTheme } from './color-theme'
import { AppearanceThemeControls } from './appearance-theme'
import { ElasticSlider } from '@/components/arc/elastic-slider/elastic-slider'
import { Stepper } from '@/components/arc/stepper/stepper'
import { ShortcutRecorder, usePlatform } from '@/components/arc/shortcut-recorder/shortcut-recorder'
import { SHORTCUT_ACTIONS, type ShortcutActionId } from './shortcut-model'
import { useShortcutPreferences } from './use-shortcuts'
import { AgentBackendSettingsSection } from './agent-backend-settings'
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
import './settings-pages.css'
import { connectionTypeLabel, knowledgeServiceName, useStoredPreference } from './settings-shared'
import { developerUiFromBuild, persistDeveloperUi, useDeveloperUi } from '@/features/chat/developer-ui'
// Batch 3 (能力 / 系统): Agent, 插件, 资料库, 数据与关于 live in their own files.
export { AgentSettingsPage } from './settings-agent-page'
export { ExtensionsSettingsPage } from './settings-plugins-page'
export { YuxiSettingsPage } from './settings-library-page'
export { DataAboutSettingsPage } from './settings-data-page'
import { SettingsGroup, SettingsPageLayout, SettingsRow, settingsSectionId, useScrollToSettingsSection } from './settings-layout'
import { PROMPT_SEND_SHORTCUT_EVENT, readPromptSendShortcut, writePromptSendShortcut, type PromptSendShortcut } from '@/components/ai-elements/prompt-send-shortcut'
import type { SettingsSectionAnchor } from '@/features/workspace/management-navigation'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { promptDialog } from '@/components/ui/prompt-dialog'



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
    return <SettingsGroup title="通知" className="fox-notification-settings"><div className="fox-notification-settings-loading"><LoaderCircle className="animate-spin" />正在读取通知设置</div></SettingsGroup>
  }

  const soundId = normalizeNotificationSoundId(preferences.soundId)
  return <SettingsGroup title="通知" className="fox-notification-settings" aside={<Button variant="outline" size="sm" onClick={() => dispatchNotificationPreview(soundId)}><BellRing />测试通知</Button>}>
    <SettingsRow title="桌面通知" description="允许 Fox 同时发送操作系统通知"><Switch checked={preferences.systemPopup} disabled={saving} onCheckedChange={(value) => void updatePreferences({ systemPopup: value })} /></SettingsRow>
    <SettingsRow title="提示音" description="为需要关注的通知播放提示音"><div className="fox-notification-sound-control">{preferences.sound && <Select value={soundId} disabled={saving} onValueChange={(value) => { const next = normalizeNotificationSoundId(value); playNotificationSound(next); void updatePreferences({ soundId: next }) }}><SelectTrigger className="fox-settings-select" aria-label="提示音"><Volume2 /><SelectValue /></SelectTrigger><SelectContent>{NOTIFICATION_SOUND_OPTIONS.map((option) => <SelectItem key={option.id} value={option.id} title={option.description}>{option.label}</SelectItem>)}</SelectContent></Select>}<Switch checked={preferences.sound} disabled={saving} onCheckedChange={(value) => void updatePreferences({ sound: value })} /></div></SettingsRow>
    <SettingsRow title="未读红点" description="有未读通知时在右上角显示红点"><Switch checked={preferences.badge} disabled={saving} onCheckedChange={(value) => void updatePreferences({ badge: value })} /></SettingsRow>
    <SettingsRow title="减少打扰" description="合并普通进度通知，审批、提问和失败仍会提醒"><Switch checked={preferences.quietProgress} disabled={saving} onCheckedChange={(value) => void updatePreferences({ quietProgress: value })} /></SettingsRow>
  </SettingsGroup>
}

export function SettingsPage({ sidebarCollapsed, onSidebar, navigate, dark, onDark }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; dark: boolean; onDark: () => void }) {
  const yuxi = useYuxiService()
  const yuxiUser = useYuxiUser(Boolean(yuxi.service?.credentialConfigured))
  const [textScale, setTextScale] = useState(() => readTextScale())
  const [colorTheme, setColorTheme] = useState<ColorTheme>(() => readColorTheme())
  const processDisplayMode = useProcessDisplayMode()
  const [profileDialogOpen, setProfileDialogOpen] = useState(false)
  const { profile, save: saveProfile } = useUserProfile({ name: yuxiUser.user?.username, avatar: yuxiUser.user?.avatar })
  const profileName = profile.name
  const profileDetail = yuxiUser.user?.departmentName ?? (yuxiUser.user ? `资料库账户 · ${yuxiUser.user.role}` : '本机用户')
  const profileFallback = profile.initial
  useEffect(() => {
    persistTextScale(textScale)
  }, [textScale])

  const changeTextScale = (next: number) => {
    setTextScale(normalizeTextScale(next))
  }

  const changeColorTheme = (next: ColorTheme) => {
    setColorTheme(persistColorTheme(next))
  }

  return (
    <WorkspacePage title="通用" subtitle="外观设置" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
      <SettingsPageLayout title="通用" description="调整个人资料、外观和通知方式。">
          <SettingsGroup title="个人资料" aside={<div className="fox-profile-setting-actions"><Button variant="outline" size="sm" onClick={() => setProfileDialogOpen(true)}><Pencil />编辑信息</Button>{!yuxiUser.user && <Button variant="outline" size="sm" onClick={() => navigate('login')}>登录账户</Button>}</div>}><div className="fox-profile-setting"><Avatar><AvatarImage src={profile.avatar} alt="" /><AvatarFallback>{profileFallback}</AvatarFallback></Avatar><span><b>{profileName}</b><small>{yuxiUser.user?.uid ?? 'fox-local'}</small><em>{profileDetail}</em></span></div></SettingsGroup>
          <SettingsGroup title="外观"><AppearanceThemeControls dark={dark} onDarkChange={(next) => { if (next !== dark) onDark() }} colorTheme={colorTheme} onColorThemeChange={changeColorTheme} /><SettingsRow title="文字大小" description="调整 Fox 各界面的文字显示比例"><div className="fox-text-scale-control"><span className="fox-text-scale-a is-small" aria-hidden="true">A</span><ElasticSlider className="fox-settings-text-slider" label="文字大小" min={TEXT_SCALE_MIN} max={TEXT_SCALE_MAX} step={1} largeStep={5} marks={[100]} value={textScale} showValue={false} format={(value) => `${value}%`} onValueChange={changeTextScale} /><span className="fox-text-scale-a is-large" aria-hidden="true">A</span><div className="fox-text-scale-stepper"><Button type="button" variant="ghost" size="icon" aria-label="缩小文字" onClick={() => changeTextScale(textScale - 5)}><Minus /></Button><label><input type="number" min={TEXT_SCALE_MIN} max={TEXT_SCALE_MAX} step={1} value={textScale} aria-label="文字大小百分比" onChange={(event) => changeTextScale(Number(event.target.value))} /><span>%</span></label><Button type="button" variant="ghost" size="icon" aria-label="放大文字" onClick={() => changeTextScale(textScale + 5)}><Plus /></Button></div></div></SettingsRow><SettingsRow title="过程详情" description="选择希望看到多少工具调用细节"><Select value={processDisplayMode} onValueChange={(value) => persistProcessDisplayMode(value as ProcessDisplayMode)}><SelectTrigger className="fox-settings-select" aria-label="过程详情"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="compact">简洁</SelectItem><SelectItem value="standard">标准</SelectItem><SelectItem value="detailed">详细</SelectItem><SelectItem value="verbose">详细</SelectItem></SelectContent></Select></SettingsRow></SettingsGroup>
          <NotificationSettingsSection />
          <SettingsGroup title="开始设置"><SettingsRow title="重新设置" description="检查模型服务、资料库连接和 Fox 的基础使用方式"><Button variant="outline" size="sm" onClick={() => navigate('onboarding')}><Sparkles />打开向导</Button></SettingsRow></SettingsGroup>
        <UserProfileDialog open={profileDialogOpen} onOpenChange={setProfileDialogOpen} profile={profile} detail={profileDetail} onSave={async (next) => { await saveProfile(next); toast.success('个人资料已更新') }} />
      </SettingsPageLayout>
    </WorkspacePage>
  )
}

/** Former 快捷键 page; its content now lives in the 对话 page. */
export function ShortcutSettingsPage({ sidebarCollapsed, onSidebar }: { sidebarCollapsed: boolean; onSidebar: () => void }) {
  return <ConversationSettingsPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} section="shortcuts" />
}

export function OnboardingPage({ navigate, onExit, dark = false }: { navigate: NavigateWorkspace; onExit: (status: 'completed' | 'skipped') => void; dark?: boolean }) {
  const model = useModelService()
  const yuxi = useYuxiService()
  const [step, setStep] = useState(0)
  const [stepDirection, setStepDirection] = useState(1)
  const [reducedMotion, setReducedMotion] = useState(() => window.matchMedia('(prefers-reduced-motion: reduce)').matches)
  useEffect(() => {
    const media = window.matchMedia('(prefers-reduced-motion: reduce)')
    const update = () => setReducedMotion(media.matches)
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])
  const [grainColors, setGrainColors] = useState<string[] | null>(null)
  useEffect(() => {
    const root = document.documentElement
    const canvas = document.createElement('canvas')
    canvas.width = canvas.height = 1
    const context = canvas.getContext('2d', { willReadFrequently: true })
    if (!context) return
    const update = () => {
      const style = getComputedStyle(root)
      const colors = ['--brand', '--muted', '--background'].map((token) => {
        context.clearRect(0, 0, 1, 1)
        context.fillStyle = token === '--brand'
          ? `color-mix(in oklch, ${style.getPropertyValue('--brand').trim()} ${style.getPropertyValue('--fox-onboarding-tint').trim() || '28%'}, ${style.getPropertyValue('--background').trim()})`
          : style.getPropertyValue(token).trim()
        context.fillRect(0, 0, 1, 1)
        return '#' + [...context.getImageData(0, 0, 1, 1).data].slice(0, 3).map((value) => value.toString(16).padStart(2, '0')).join('')
      })
      setGrainColors(colors)
    }
    update()
    const observer = new MutationObserver(update)
    observer.observe(root, { attributes: true, attributeFilter: ['class', 'data-color-theme', 'style'] })
    return () => observer.disconnect()
  }, [dark])
  const steps = [
    { title: '连接模型服务', short: '模型', description: '配置 Fox 原生 Agent 使用的模型、API 协议与上下文能力。', ready: Boolean(model.service), action: () => navigate('settings-models'), actionLabel: model.service ? '检查模型配置' : '配置模型服务' },
    { title: '连接资料库服务', short: '资料库', description: '连接本机或局域网服务，启用远程专家、资料库和知识图谱。', ready: Boolean(yuxi.service), action: () => navigate('settings-yuxi'), actionLabel: yuxi.service ? '检查资料库连接' : '配置资料库服务' },
    { title: '开始使用 Fox', short: '开始', description: '模型服务是原生专家的必要条件；资料库服务可以稍后连接。', ready: Boolean(model.service), action: () => onExit('completed'), actionLabel: '开始使用' },
  ]
  const current = steps[step]
  const statusCopy = step === 0
    ? model.service ? `${model.service.name} · ${model.service.modelId}` : '尚未配置模型服务，原生专家无法发送消息'
    : step === 1
      ? yuxi.service ? `${knowledgeServiceName(yuxi.service.name)} · ${yuxi.service.baseUrl}` : '尚未配置资料库服务，资料库与远程专家不可用'
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
    {grainColors && <Grainient
      className="fox-onboarding-background"
      color1={grainColors[0]}
      color2={grainColors[1]}
      color3={grainColors[2]}
      timeSpeed={reducedMotion ? 0 : 0.6}
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
    />}
    <Button variant="outline" size="sm" className="fox-onboarding-skip" onClick={() => onExit('skipped')}>暂时跳过</Button>
    <div className="fox-onboarding-shell">
      <section className="fox-onboarding-main">
        <header className="fox-onboarding-brand"><span><img src="/mascot/fox/idle/fox_sit_nicely.png" alt="" />Fox</span></header>
        <main className="fox-onboarding-form">
          <div className="fox-onboarding-card">
            <Stepper className="fox-onboarding-stepper" label="Fox 设置进度" completeLabel="设置完成" steps={steps.map((item, index) => ({ id: String(index), label: item.short }))} current={step} details="current" onStepSelect={(index) => { setStepDirection(index >= step ? 1 : -1); setStep(index) }} />
            <div className="fox-onboarding-step-viewport">
              <AnimatePresence initial={false} mode="sync" custom={stepDirection}>
                <motion.div
                  key={step}
                  className="fox-onboarding-step-content"
                  custom={stepDirection}
                  variants={{
                    enter: (direction: number) => ({ x: reducedMotion ? 0 : direction >= 0 ? '100%' : '-100%', opacity: 0 }),
                    active: { x: '0%', opacity: 1 },
                    leave: (direction: number) => ({ x: reducedMotion ? 0 : direction >= 0 ? '-100%' : '100%', opacity: 0 }),
                  }}
                  initial="enter"
                  animate="active"
                  exit="leave"
                  transition={{ duration: reducedMotion ? 0 : 0.4, ease: [0.22, 1, 0.36, 1] }}
                >
                  <header><h1>{current.title}</h1><p>{current.description}</p></header>
                  <div className={`fox-onboarding-status ${current.ready ? 'is-ready' : ''}`}><span>{current.ready ? <Check /> : <LoaderCircle />}</span><div><b>{current.ready ? '当前状态正常' : '尚未完成'}</b><small>{statusCopy}</small></div><Badge variant={current.ready ? 'secondary' : 'outline'}>{current.ready ? '可使用' : '待配置'}</Badge></div>
                  {step < 2 && <Button variant="outline" className="fox-onboarding-configure" onClick={current.action}>{current.actionLabel}<ChevronRight /></Button>}
                </motion.div>
              </AnimatePresence>
            </div>
            <footer><Button variant="outline" className="fox-onboarding-prev" disabled={step === 0} onClick={goBack}>上一步</Button><BorderBeam active={step === 2 && Boolean(model.service)} className="fox-onboarding-next-beam" colorVariant="colorful" size="md" strength={0.7} theme={dark ? 'dark' : 'light'}><Button className="fox-onboarding-next" disabled={step === 2 && !model.service} onClick={() => step === 2 ? current.action() : advance()}><span className="fox-onboarding-button-label">{step === 2 ? '开始使用' : '下一步'}</span><ChevronRight /></Button></BorderBeam></footer>
            {step === 2 && !model.service && <button type="button" className="fox-onboarding-skip-link" onClick={() => onExit('skipped')}>暂时跳过，稍后在设置中配置</button>}
          </div>
        </main>
      </section>
      <aside className="fox-onboarding-visual">
        <div className="fox-onboarding-visual-media"><img src="/mascot/fox/status/fox_sayhi.png" alt="Fox 卡通形象" /><div><strong>Fox Desktop</strong><span>轻量、高效的桌面专家工作台</span></div><BorderBeam className="fox-onboarding-enter" colorVariant="colorful" size="md" strength={0.7} theme={dark ? 'dark' : 'light'}><Button className="fox-onboarding-enter-button" size="sm" variant="outline" onClick={() => onExit(model.service ? 'completed' : 'skipped')}><span className="fox-onboarding-button-label">开始使用</span><ChevronRight /></Button></BorderBeam></div>
      </aside>
    </div>
  </div>
}

export { ModelAgentPage as ModelProvidersPage } from './model-agent/model-agent-page'

function usePromptSendShortcutPreference() {
  const [value, setValue] = useState<PromptSendShortcut>(() => readPromptSendShortcut())
  useEffect(() => {
    const sync = () => setValue(readPromptSendShortcut())
    window.addEventListener(PROMPT_SEND_SHORTCUT_EVENT, sync)
    window.addEventListener('storage', sync)
    return () => {
      window.removeEventListener(PROMPT_SEND_SHORTCUT_EVENT, sync)
      window.removeEventListener('storage', sync)
    }
  }, [])
  const update = (next: PromptSendShortcut) => setValue(writePromptSendShortcut(next))
  return [value, update] as const
}

function SendShortcutHint({ mode }: { mode: PromptSendShortcut }) {
  return <span className="fox-settings-hint" data-testid="send-shortcut-hint">换行改用
    {mode === 'enter'
      ? <KbdGroup><Kbd>Shift</Kbd><Kbd>Enter</Kbd></KbdGroup>
      : <Kbd>Enter</Kbd>}
  </span>
}

/** 快捷键 group shared by the 对话 page; same recorder, conflict and reset logic as before. */
function ShortcutSettingsGroup() {
  const { preferences, setBinding, restoreDefaults } = useShortcutPreferences()
  const platform = usePlatform()
  const [error, setError] = useState<string | null>(null)
  const [resetOpen, setResetOpen] = useState(false)
  const customized = SHORTCUT_ACTIONS.some((action) => preferences[action.id] !== action.defaultShortcut)
  const saveBinding = (actionId: ShortcutActionId, shortcut: string | null, replaceConflicts: boolean) => {
    try {
      setBinding(actionId, shortcut, replaceConflicts)
      setError(null)
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '快捷键保存失败，请重试')
      return false
    }
  }
  const reset = () => {
    try {
      restoreDefaults()
      setError(null)
      setResetOpen(false)
      toast.success('快捷键已恢复默认')
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '恢复默认失败，请重试')
      setResetOpen(false)
    }
  }
  return <SettingsGroup id={settingsSectionId('shortcuts')} title="快捷键" aside={<Button type="button" variant="ghost" size="sm" disabled={!customized} onClick={() => setResetOpen(true)}><RotateCcw />恢复默认</Button>}>
    {SHORTCUT_ACTIONS.map((action) => <SettingsRow key={action.id} title={action.label} description={action.description}>
      <ShortcutRecorder
        className="fox-settings-shortcut"
        label={`${action.label}快捷键`}
        hideLabel
        platform={platform}
        value={preferences[action.id]}
        resetValue={action.defaultShortcut}
        placeholder="未设置，点击录制"
        bindings={SHORTCUT_ACTIONS.filter((item) => item.id !== action.id && preferences[item.id]).map((item) => ({ shortcut: preferences[item.id]!, label: item.label }))}
        onValueChange={(value, details) => saveBinding(action.id, value, Boolean(details.replaced))}
      />
    </SettingsRow>)}
    {error && <p className="fox-settings-error" role="alert">{error}</p>}
    <p className="fox-settings-footnote">点击快捷键后按下新的组合键即可修改，录制时按 Esc 取消、Backspace 清除；单项恢复默认在悬停时出现。输入法选字期间不会触发快捷键。</p>
    <Dialog open={resetOpen} onOpenChange={setResetOpen}>
      <DialogContent><DialogHeader><DialogTitle>恢复默认快捷键</DialogTitle><DialogDescription>所有快捷键将恢复为 Fox 默认组合键，已清除的快捷键也会重新启用。</DialogDescription></DialogHeader><DialogFooter><Button type="button" variant="outline" onClick={() => setResetOpen(false)}>取消</Button><Button type="button" onClick={reset}>恢复默认</Button></DialogFooter></DialogContent>
    </Dialog>
  </SettingsGroup>
}

export function ConversationSettingsPage({ sidebarCollapsed, onSidebar, section }: { sidebarCollapsed: boolean; onSidebar: () => void; section?: SettingsSectionAnchor }) {
  const [spellcheck, setSpellcheck] = useStoredPreference('fox.preferences.spellcheck', true)
  const developerUi = useDeveloperUi()
  const [sendShortcut, setSendShortcut] = usePromptSendShortcutPreference()
  useScrollToSettingsSection(section)
  // 「消息导航」 has no backing state in the chat view yet, so it is not offered
  // as a switch; 「文件附件」 was a read-only label and is gone.
  return <WorkspacePage title="对话" subtitle="消息输入与快捷键" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout title="对话" description="调整消息输入方式和常用快捷键。">
      <SettingsGroup title="消息输入">
        <SettingsRow title="发送方式" description="中文输入法选字时，Enter 只确认候选字">
          <Tabs className="fox-settings-segmented" value={sendShortcut} onValueChange={(value) => { if (value === 'enter' || value === 'mod-enter') setSendShortcut(value) }}>
            <TabsList aria-label="发送方式">
              <TabsTrigger value="enter">Enter 发送</TabsTrigger>
              <TabsTrigger value="mod-enter">Ctrl/Cmd + Enter 发送</TabsTrigger>
            </TabsList>
          </Tabs>
          <SendShortcutHint mode={sendShortcut} />
        </SettingsRow>
        <SettingsRow title="拼写检查" description="使用系统 WebView 的拼写检查能力">
          <Switch checked={spellcheck} onCheckedChange={setSpellcheck} aria-label="拼写检查" />
        </SettingsRow>
      </SettingsGroup>
      <SettingsGroup title="开发者">
        <SettingsRow title="开发者模式" description={developerUiFromBuild() ? '已由环境变量 FOX_DEV_UI 打开' : '在输入框上方显示外部 Agent 的调试信息：锁定的后端、能力声明、配置来源与导入、本地预览'}>
          <Switch checked={developerUi} disabled={developerUiFromBuild()} onCheckedChange={persistDeveloperUi} aria-label="开发者模式" />
        </SettingsRow>
      </SettingsGroup>
      <ShortcutSettingsGroup />
    </SettingsPageLayout>
  </WorkspacePage>
}

export function ServicePage({ sidebarCollapsed, onSidebar, navigate }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace }) {
  const yuxi = useYuxiService()
  const [name, setName] = useState('本机资料库')
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
    if (result) toast.success(result.authenticated ? '连接成功，访问凭证已验证' : `资料库服务连接成功 · ${result.version ?? '版本未知'}`)
  }

  const saveConnection = async () => {
    const saved = await yuxi.save({ name, baseUrl, accessToken: token || undefined })
    if (!saved) return
    await yuxi.test(saved.baseUrl)
    setToken('')
    toast.success('资料库服务配置已保存')
    returnFromService()
  }

  const result = yuxi.testResult
  return (
    <WorkspacePage title="连接资料库" subtitle="服务配置" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={returnFromService}>
      <div className="fox-settings-layout"><main><header><span>服务连接</span><h1>资料库服务</h1><p>连接本机、局域网或远程资料库服务，为 Fox 提供远程专家、资料库和知识图谱能力。</p></header><Card className="fox-settings-section fox-service-form"><div className="fox-settings-section-head"><div><h2>连接配置</h2><p>Fox 首版仅保存一个资料库服务地址。</p></div><Server /></div><div className="fox-settings-form-grid"><label>服务名称<Input value={name} onChange={(event) => setName(event.target.value)} /></label><label>API 地址 <b>*</b><Input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="http://127.0.0.1:5050" /></label><small className="fox-field-help">远程示例：https://knowledge.example.com，Fox 会检测 /api/system/health。</small><label>访问令牌<Input type="password" value={token} onChange={(event) => setToken(event.target.value)} placeholder={yuxi.service?.credentialConfigured ? '已保存；留空保留原值' : '可选：API Key 或登录 Token'} /></label></div>{result ? <Card className="fox-connection-result"><span><Server /></span><p><b>{result.authenticated ? '已验证' : '已连通'}</b><small>{result.version ?? '未知版本'} · {result.latencyMs} ms · {connectionTypeLabel(result.connectionType)}</small></p><Badge variant="secondary">{result.authenticated ? '已验证' : '可用'}</Badge></Card> : yuxi.error ? <Card className="fox-connection-result is-error"><span><Server /></span><p><b>连接测试失败</b><small>{yuxi.error}</small></p><Badge variant="destructive">不可用</Badge></Card> : null}<div className="fox-setting-actions"><Button variant="outline" disabled={yuxi.testing || !baseUrl.trim()} onClick={() => void testConnection()}>{yuxi.testing && <LoaderCircle className="animate-spin" />}测试连接</Button><Button disabled={yuxi.saving || !name.trim() || !baseUrl.trim()} onClick={() => void saveConnection()}>{yuxi.saving && <LoaderCircle className="animate-spin" />}保存配置</Button></div><small className="fox-settings-security-note"><Shield />Token 使用系统凭证库保存，不写入 Fox 数据库。</small></Card></main></div>
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
      <div className="fox-settings-layout"><main><header><span>模型连接</span><h1>模型服务</h1><p>配置 Fox 原生 Agent 使用的 OpenAI-compatible 或 Anthropic Messages 模型服务。</p></header><Card className="fox-settings-section fox-service-form"><div className="fox-settings-section-head"><div><h2>运行配置</h2><p>模型服务与资料库服务相互独立。</p></div><Bot /></div><div className="fox-settings-form-grid"><label>服务名称<Input value={name} onChange={(event) => setName(event.target.value)} /></label><label>API 协议 <b>*</b><Select value={apiType} onValueChange={(value) => setApiType(value as typeof apiType)}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="openai-completions">OpenAI-compatible</SelectItem><SelectItem value="anthropic-messages">Anthropic Messages</SelectItem></SelectContent></Select></label><label>API 地址 <b>*</b><Input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder={apiType === 'anthropic-messages' ? 'https://api.minimaxi.com/anthropic' : 'http://127.0.0.1:11434/v1'} /></label><small className="fox-field-help">{apiType === 'anthropic-messages' ? '填写 Anthropic API 根路径；MiniMax 国内订阅可使用 https://api.minimaxi.com/anthropic。' : '填写 OpenAI-compatible API 根路径，Fox 会检测 /models。'}</small><label>模型 ID <b>*</b><Input value={modelId} onChange={(event) => setModelId(event.target.value)} placeholder={apiType === 'anthropic-messages' ? '例如 MiniMax-M3' : '例如 qwen3:8b 或 deepseek-chat'} /></label><div className="fox-model-limits"><label>上下文长度<Input inputMode="numeric" value={contextWindow} onChange={(event) => setContextWindow(event.target.value)} /></label><label>最大输出<Input inputMode="numeric" value={maxOutputTokens} onChange={(event) => setMaxOutputTokens(event.target.value)} /></label></div><div className="fox-setting-row"><span><b>图片理解</b><small>仅在当前模型明确支持视觉理解时开启</small></span><Switch checked={supportsImageInput} onCheckedChange={setSupportsImageInput} /></div><label>API Key<Input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder={model.service?.credentialConfigured ? '已保存；留空保留原值' : apiType === 'anthropic-messages' ? 'Anthropic Messages 必填' : '本机服务可留空'} /></label></div>{result ? <Card className="fox-connection-result"><span><Bot /></span><p><b>连接测试成功</b><small>{result.models.length} 个模型 · {result.latencyMs} ms · {connectionTypeLabel(result.connectionType)}</small></p><Badge variant="secondary">可用</Badge></Card> : model.error ? <Card className="fox-connection-result is-error"><span><Bot /></span><p><b>连接测试失败</b><small>{model.error}</small></p><Badge variant="destructive">不可用</Badge></Card> : null}<div className="fox-setting-actions"><Button variant="outline" disabled={model.testing || !baseUrl.trim() || !modelId.trim()} onClick={() => void testConnection()}>{model.testing && <LoaderCircle className="animate-spin" />}测试连接</Button><Button disabled={model.saving || !name.trim() || !baseUrl.trim() || !modelId.trim()} onClick={() => void saveConnection()}>{model.saving && <LoaderCircle className="animate-spin" />}保存配置</Button></div><small className="fox-settings-security-note"><Shield />API Key 使用系统凭证库保存，不写入 Fox 数据库。</small></Card></main></div>
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
    <WorkspacePage title="登录账户" subtitle="账户身份验证" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} onBack={returnFromLogin}>
      <div className="fox-login-page"><section><img src="/mascot/fox_magic.png" alt="" /><span>欢迎回来</span><h1>登录账户</h1><p>登录后将同步你有权访问的远程专家与资料库。</p></section><Card><header><span><LogIn /></span><div><h2>使用资料库账户</h2><p>连接到已配置的资料库服务</p></div></header><label>用户名<Input value={username} onChange={(event) => setUsername(event.target.value)} /></label><label>密码<Input type="password" value={password} onChange={(event) => setPassword(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') void login() }} /></label>{error && <p className="fox-setting-error">{error}</p>}<Button size="lg" disabled={busy || !username || !password} onClick={() => void login()}>{busy && <LoaderCircle className="animate-spin" />}登录并进入 Fox</Button><Separator /><Button variant="outline" size="lg" onClick={openService}><ExternalLink />检查连接</Button></Card></div>
    </WorkspacePage>
  )
}
