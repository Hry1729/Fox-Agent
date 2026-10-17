import { FOX_ASSISTANT_AVATAR } from '../components/FoxAssistantAvatar'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuShortcut, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ClipboardPaste, Copy, FolderOpen, Minus, Redo2, Sparkles, Undo2, X, ZoomIn, ZoomOut } from 'lucide-react'
import { AlignLeft, AnimatedIconTrigger, Maximize, MessageCirclePlus, Minimize, PanelLeft, Settings } from '@/components/fox-ui/animated-icons'
import { memo, useEffect, useState, type ReactNode } from 'react'
import { detectShortcutPlatform, formatShortcut, type ShortcutActionId } from '@/features/settings/shortcut-model'
import { useShortcutPreferences } from '@/features/settings/use-shortcuts'

function WindowTitlebar({ leftSidebarCollapsed, onNewChat, onOpenProject, onSettings, onAbout, onToggleSidebar, onZoom }: {
  leftSidebarCollapsed: boolean
  onNewChat: () => void
  onOpenProject: () => void
  onSettings: () => void
  onAbout: () => void
  onToggleSidebar: () => void
  onZoom: (delta: number) => void
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
        <strong>Fox</strong>
        <div className="fox-titlebar-sidebar-controls" aria-label="标题栏控制">
          <AnimatedIconTrigger><AppMenu onNewChat={onNewChat} onOpenProject={onOpenProject} onSettings={onSettings} onAbout={onAbout} leftSidebarCollapsed={leftSidebarCollapsed} onToggleSidebar={onToggleSidebar} onZoom={onZoom} onClose={() => void windowAction('close')} shortcutHint={shortcutHint} /></AnimatedIconTrigger>
          <AnimatedIconTrigger><button type="button" aria-label={leftSidebarCollapsed ? '展开左侧边栏' : '收起左侧边栏'} title={leftSidebarCollapsed ? '展开左侧边栏' : '收起左侧边栏'} aria-pressed={!leftSidebarCollapsed} onClick={onToggleSidebar}><PanelLeft /></button></AnimatedIconTrigger>
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

function WindowMenuLabel({ children }: { children: string }) {
  return <DropdownMenuLabel className="fox-window-menu-label">{children}</DropdownMenuLabel>
}

/** One icon menu for everything the native-style bar used to spell out as
 *  文件 / 编辑 / 视图 / 帮助, so the title bar stays two controls wide. */
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
      <DropdownMenuTrigger asChild><button type="button" className="fox-titlebar-menu-trigger" aria-label="应用菜单" title="应用菜单"><AlignLeft /></button></DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={5} className="fox-window-menu-content fox-app-menu">
        <WindowMenuLabel>文件</WindowMenuLabel>
        <DropdownMenuItem onSelect={onNewChat}><MessageCirclePlus />新建对话{shortcutHint('new-conversation')}</DropdownMenuItem>
        <DropdownMenuItem onSelect={onOpenProject}><FolderOpen />打开项目{shortcutHint('add-project')}</DropdownMenuItem>
        <DropdownMenuItem onSelect={onSettings}><Settings />设置{shortcutHint('open-settings')}</DropdownMenuItem>
        <DropdownMenuSeparator />
        <WindowMenuLabel>编辑</WindowMenuLabel>
        <DropdownMenuItem onSelect={() => document.execCommand('undo')}><Undo2 />撤销<DropdownMenuShortcut>Ctrl Z</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuItem onSelect={() => document.execCommand('redo')}><Redo2 />重做<DropdownMenuShortcut>Ctrl Y</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuItem onSelect={() => document.execCommand('copy')}><Copy />复制<DropdownMenuShortcut>Ctrl C</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuItem onSelect={() => document.execCommand('paste')}><ClipboardPaste />粘贴<DropdownMenuShortcut>Ctrl V</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuSeparator />
        <WindowMenuLabel>视图</WindowMenuLabel>
        <DropdownMenuItem onSelect={onToggleSidebar}><PanelLeft />{leftSidebarCollapsed ? '显示侧栏' : '隐藏侧栏'}{shortcutHint('toggle-sidebar')}</DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onZoom(0.1)}><ZoomIn />放大<DropdownMenuShortcut>Ctrl +</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onZoom(-0.1)}><ZoomOut />缩小<DropdownMenuShortcut>Ctrl -</DropdownMenuShortcut></DropdownMenuItem>
        <DropdownMenuSeparator />
        <WindowMenuLabel>帮助</WindowMenuLabel>
        <DropdownMenuItem onSelect={onAbout}><Sparkles />关于 Fox</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={onClose}>退出</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
