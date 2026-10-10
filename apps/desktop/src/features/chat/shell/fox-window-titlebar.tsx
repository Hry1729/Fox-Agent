import { FOX_ASSISTANT_AVATAR } from '../components/FoxAssistantAvatar'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuShortcut, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Copy, FolderOpen, MessageSquarePlus, Minus, PanelLeft, Settings, Sparkles, Square, X } from 'lucide-react'
import { type ReactNode, memo, useEffect, useState } from 'react'

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
      void appWindow.onResized(() => { void syncMaximized() }).then((cleanup) => { unlisten = cleanup })
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
        <div className="fox-titlebar-sidebar-controls" aria-label="侧边栏控制">
          <button type="button" aria-label={leftSidebarCollapsed ? '展开左侧边栏' : '收起左侧边栏'} title={leftSidebarCollapsed ? '展开左侧边栏' : '收起左侧边栏'} aria-pressed={!leftSidebarCollapsed} onClick={onToggleSidebar}><PanelLeft /></button>
        </div>
        <nav aria-label="应用菜单">
          <WindowMenu label="文件">
            <DropdownMenuItem onSelect={onNewChat}><MessageSquarePlus />新建对话<DropdownMenuShortcut>Ctrl N</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={onOpenProject}><FolderOpen />打开项目<DropdownMenuShortcut>Ctrl O</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={onSettings}><Settings />设置<DropdownMenuShortcut>Ctrl ,</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => void windowAction('close')}>退出</DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="编辑">
            <DropdownMenuItem onSelect={() => document.execCommand('undo')}>撤销<DropdownMenuShortcut>Ctrl Z</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={() => document.execCommand('redo')}>重做<DropdownMenuShortcut>Ctrl Y</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => document.execCommand('copy')}>复制<DropdownMenuShortcut>Ctrl C</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={() => document.execCommand('paste')}>粘贴<DropdownMenuShortcut>Ctrl V</DropdownMenuShortcut></DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="视图">
            <DropdownMenuItem onSelect={onToggleSidebar}><PanelLeft />切换侧边栏<DropdownMenuShortcut>Ctrl B</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => onZoom(0.1)}>放大<DropdownMenuShortcut>Ctrl +</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onZoom(-0.1)}>缩小<DropdownMenuShortcut>Ctrl -</DropdownMenuShortcut></DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="帮助">
            <DropdownMenuItem onSelect={onAbout}><Sparkles />关于 Fox</DropdownMenuItem>
          </WindowMenu>
        </nav>
      </div>
      <div className="fox-window-controls">
        <button type="button" aria-label="最小化" onClick={() => void windowAction('minimize')}><Minus /></button>
        <button type="button" aria-label={maximized ? '还原窗口' : '最大化'} title={maximized ? '还原窗口' : '最大化'} onClick={() => void windowAction('maximize')}>{maximized ? <Copy /> : <Square />}</button>
        <button type="button" className="is-close" aria-label="关闭" onClick={() => void windowAction('close')}><X /></button>
      </div>
    </header>
  )
}

export const MemoizedWindowTitlebar = memo(WindowTitlebar)

function WindowMenu({ label, children }: { label: string; children: ReactNode }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild><button type="button">{label}</button></DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={3} className="fox-window-menu-content">{children}</DropdownMenuContent>
    </DropdownMenu>
  )
}
