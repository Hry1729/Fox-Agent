import { FOX_ASSISTANT_AVATAR } from '../components/FoxAssistantAvatar'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuPortal, DropdownMenuSub, DropdownMenuSubTrigger, DropdownMenuSubContent, DropdownMenuSeparator, DropdownMenuShortcut, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ClipboardPaste, Copy, FolderOpen, Minus, Redo2, Sparkles, Undo2, X, ZoomIn, ZoomOut } from 'lucide-react'
import { AlignLeft, AnimatedIconTrigger, ArrowLeft, ArrowRight, Maximize, MessageCirclePlus, Minimize, PanelLeft, Settings } from '@/components/fox-ui/animated-icons'
import { memo, useEffect, useState, type ReactNode } from 'react'
import { detectShortcutPlatform, formatShortcut, type ShortcutActionId } from '@/features/settings/shortcut-model'
import { useShortcutPreferences } from '@/features/settings/use-shortcuts'

function WindowTitlebar({ leftSidebarCollapsed, onNewChat, onOpenProject, onSettings, onAbout, onToggleSidebar, onZoom, canGoBack = false, canGoForward = false, onGoBack, onGoForward }: {
  leftSidebarCollapsed: boolean
  onNewChat: () => void
  onOpenProject: () => void
  onSettings: () => void
  onAbout: () => void
  onToggleSidebar: () => void
  onZoom: (delta: number) => void
  canGoBack?: boolean
  canGoForward?: boolean
  onGoBack?: () => void
  onGoForward?: () => void
}) {
  const [maximized, setMaximized] = useState(false)
  const { preferences } = useShortcutPreferences()
  const platform = detectShortcutPlatform()
  const shortcutHint = (actionId: ShortcutActionId) => {
    const binding = preferences[actionId]
    if (!binding) return null
    const label = formatShortcut(binding, platform)
    return <DropdownMenuShortcut aria-label={label.spoken}>{label.text}</DropdownMenuShortcut>
  }

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    try {
      const appWindow = getCurrentWindow()
      const syncMaximized = async () => {
        try {
          const value = await appWindow.isMaximized()
          if (!disposed) setMaximized(value)
        } catch {
          // Browser preview has no native window state.
        }
      }
      void syncMaximized()
      void appWindow.onResized(() => { void syncMaximized() }).then((cleanup) => { if (disposed) cleanup(); else unlisten = cleanup }).catch(() => { /* Browser preview has no native events. */ })
    } catch {
      // Browser preview has no native window state.
    }
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  const windowAction = async (action: 'minimize' | 'maximize' | 'close') => {
    try {
      const appWindow = getCurrentWindow()
      if (action === 'minimize') await appWindow.minimize()
      if (action === 'maximize') {
        await appWindow.toggleMaximize()
        setMaximized(await appWindow.isMaximized())
      }
      if (action === 'close') await appWindow.close()
    } catch {
      // Browser preview has no native window; buttons remain harmless there.
    }
  }

  return (
    <header className="fox-window-titlebar" data-tauri-drag-region>
      <div className="fox-window-product" data-tauri-drag-region>
        <img src={FOX_ASSISTANT_AVATAR} alt="" />
        <div className="fox-titlebar-sidebar-controls" aria-label="标题栏控制">
          <AppMenu onNewChat={onNewChat} onOpenProject={onOpenProject} onSettings={onSettings} onAbout={onAbout} leftSidebarCollapsed={leftSidebarCollapsed} onToggleSidebar={onToggleSidebar} onZoom={onZoom} onClose={() => void windowAction('close')} shortcutHint={shortcutHint} />
          <AnimatedIconTrigger><button type="button" aria-label={leftSidebarCollapsed ? '展开左侧边栏' : '收起左侧边栏'} title={leftSidebarCollapsed ? '展开左侧边栏' : '收起左侧边栏'} aria-pressed={!leftSidebarCollapsed} onClick={onToggleSidebar}><PanelLeft /></button></AnimatedIconTrigger>
          <AnimatedIconTrigger animateOnHover={canGoBack} animateOnTap={canGoBack}><button type="button" aria-label="后退" title="后退" disabled={!canGoBack} onClick={onGoBack}><ArrowLeft /></button></AnimatedIconTrigger>
          <AnimatedIconTrigger animateOnHover={canGoForward} animateOnTap={canGoForward}><button type="button" aria-label="前进" title="前进" disabled={!canGoForward} onClick={onGoForward}><ArrowRight /></button></AnimatedIconTrigger>
        </div>
      </div>
      <div className="fox-window-controls">
        <button type="button" aria-label="最小化" onClick={() => void windowAction('minimize')}><Minus /></button>
        <AnimatedIconTrigger><button type="button" aria-label={maximized ? '还原窗口' : '最大化'} title={maximized ? '还原窗口' : '最大化'} onClick={() => void windowAction('maximize')}>{maximized ? <Minimize /> : <Maximize />}</button></AnimatedIconTrigger>
        <button type="button" className="is-close" aria-label="关闭" onClick={() => void windowAction('close')}><X /></button>
      </div>
    </header>
  )
}

export const MemoizedWindowTitlebar = memo(WindowTitlebar)

function AppMenu({ leftSidebarCollapsed, onNewChat, onOpenProject, onSettings, onAbout, onToggleSidebar, onZoom, onClose, shortcutHint }: {
  leftSidebarCollapsed: boolean
  onNewChat: () => void
  onOpenProject: () => void
  onSettings: () => void
  onAbout: () => void
  onToggleSidebar: () => void
  onZoom: (delta: number) => void
  onClose: () => void
  shortcutHint: (actionId: ShortcutActionId) => ReactNode
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild><AnimatedIconTrigger><button type="button" className="fox-titlebar-menu-trigger" aria-label="应用菜单" title="应用菜单"><AlignLeft /></button></AnimatedIconTrigger></DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={5} className="fox-window-menu-content fox-app-menu">
        <DropdownMenuSub><DropdownMenuSubTrigger>文件</DropdownMenuSubTrigger><DropdownMenuPortal><DropdownMenuSubContent className="fox-window-menu-content" collisionPadding={8}>
        <DropdownMenuItem onSelect={onNewChat}><MessageCirclePlus />新建对话{shortcutHint('new-conversation')}</DropdownMenuItem>
        <DropdownMenuItem onSelect={onOpenProject}><FolderOpen />打开项目{shortcutHint('add-project')}</DropdownMenuItem>
        <DropdownMenuItem onSelect={onSettings}><Settings />设置{shortcutHint('open-settings')}</DropdownMenuItem>
        <DropdownMenuSeparator /><DropdownMenuItem onSelect={onClose}>退出</DropdownMenuItem>
        </DropdownMenuSubContent></DropdownMenuPortal></DropdownMenuSub>
        <DropdownMenuSub><DropdownMenuSubTrigger>编辑</DropdownMenuSubTrigger><DropdownMenuPortal><DropdownMenuSubContent className="fox-window-menu-content" collisionPadding={8}>
        <DropdownMenuItem onSelect={() => document.execCommand('undo')}><Undo2 />撤销<DropdownMenuShortcut>Ctrl Z</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuItem onSelect={() => document.execCommand('redo')}><Redo2 />重做<DropdownMenuShortcut>Ctrl Y</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuItem onSelect={() => document.execCommand('copy')}><Copy />复制<DropdownMenuShortcut>Ctrl C</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuItem onSelect={() => document.execCommand('paste')}><ClipboardPaste />粘贴<DropdownMenuShortcut>Ctrl V</DropdownMenuShortcut></DropdownMenuItem>
        </DropdownMenuSubContent></DropdownMenuPortal></DropdownMenuSub>
        <DropdownMenuSub><DropdownMenuSubTrigger>视图</DropdownMenuSubTrigger><DropdownMenuPortal><DropdownMenuSubContent className="fox-window-menu-content" collisionPadding={8}>
        <DropdownMenuItem onSelect={onToggleSidebar}><PanelLeft />{leftSidebarCollapsed ? '显示侧栏' : '隐藏侧栏'}{shortcutHint('toggle-sidebar')}</DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onZoom(0.1)}><ZoomIn />放大<DropdownMenuShortcut>Ctrl +</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onZoom(-0.1)}><ZoomOut />缩小<DropdownMenuShortcut>Ctrl -</DropdownMenuShortcut></DropdownMenuItem>
        </DropdownMenuSubContent></DropdownMenuPortal></DropdownMenuSub>
        <DropdownMenuSub><DropdownMenuSubTrigger>帮助</DropdownMenuSubTrigger><DropdownMenuPortal><DropdownMenuSubContent className="fox-window-menu-content" collisionPadding={8}>
        <DropdownMenuItem onSelect={onAbout}><Sparkles />关于 Fox</DropdownMenuItem>
        </DropdownMenuSubContent></DropdownMenuPortal></DropdownMenuSub>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
