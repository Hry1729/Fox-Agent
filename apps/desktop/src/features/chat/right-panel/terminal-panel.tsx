import { FitAddon } from '@xterm/addon-fit'
import { Terminal as Xterm, type ITheme } from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import { RotateCcw, Terminal } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { RightPanelEmpty } from './right-panel-empty'
import './terminal-panel.css'

type ShellState = { type: 'starting' } | { type: 'ready' } | { type: 'exited'; code: number | null }
  | { type: 'error'; message: string }

export function TerminalPanel({ conversationId, projectRoot, active }: { conversationId: string; projectRoot: string; active: boolean }) {
  if (!desktopRuntimeAvailable) return <RightPanelEmpty icon={Terminal}>请在 Fox 桌面版中打开 PowerShell。</RightPanelEmpty>
  return <PowerShellTerminal conversationId={conversationId} projectRoot={projectRoot} active={active} />
}

function PowerShellTerminal({ conversationId, projectRoot, active }: { conversationId: string; projectRoot: string; active: boolean }) {
  const host = useRef<HTMLDivElement>(null)
  const instance = useRef<Xterm | null>(null)
  const [shell, setShell] = useState<ShellState>({ type: 'starting' })
  const [attempt, setAttempt] = useState(0)

  useEffect(() => {
    const element = host.current!
    const terminal = new Xterm({ screenReaderMode: true, lineHeight: 1.2 })
    const fit = new FitAddon()
    instance.current = terminal
    terminal.loadAddon(fit)
    terminal.open(element)
    terminal.textarea?.setAttribute('aria-label', 'PowerShell 终端')

    const fitVisible = () => {
      if (element.clientWidth && element.clientHeight) fit.fit()
    }
    const syncTheme = () => {
      const style = getComputedStyle(element)
      terminal.options.fontFamily = style.fontFamily
      terminal.options.fontSize = Number.parseFloat(style.fontSize)
      terminal.options.theme = terminalTheme(style)
      fitVisible()
    }
    syncTheme()
    const observer = new ResizeObserver(fitVisible)
    observer.observe(element)
    const themeObserver = new MutationObserver(syncTheme)
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'style', 'data-color-theme'] })

    let id: string | undefined
    let disposed = false
    let exited = false
    let pending = Promise.resolve()
    const beforeStart: string[] = []
    const enqueue = (send: (sessionId: string) => Promise<void>) => {
      pending = pending.then(async () => {
        if (id && !disposed && !exited) await send(id)
      }).catch(error => {
        if (!disposed && !exited) {
          terminal.options.disableStdin = true
          setShell({ type: 'error', message: String(error instanceof Error ? error.message : error) })
        }
      })
    }
    const sendInput = (data: string) => enqueue(session => desktopClient.projectTerminalWrite(session, data))
    const input = terminal.onData(data => {
      // ConPTY can ask xterm for its cursor position before the start command returns.
      if (id) sendInput(data)
      else beforeStart.push(data)
    })
    const resizing = terminal.onResize(size => {
      if (id) enqueue(session => desktopClient.projectTerminalResize(session, size))
    })
    const startingSize = { cols: terminal.cols, rows: terminal.rows }
    void desktopClient.projectTerminalStart(conversationId, startingSize, event => {
      if (disposed) return
      if (event.type === 'output') terminal.write(Uint8Array.from(event.data))
      else {
        exited = true
        terminal.options.disableStdin = true
        setShell({ type: 'exited', code: event.code })
      }
    }).then(sessionId => {
      if (disposed) {
        void desktopClient.projectTerminalClose(sessionId).catch(() => {})
        return
      }
      id = sessionId
      if (exited) return
      beforeStart.splice(0).forEach(sendInput)
      if (startingSize.cols !== terminal.cols || startingSize.rows !== terminal.rows) {
        enqueue(session => desktopClient.projectTerminalResize(session, { cols: terminal.cols, rows: terminal.rows }))
      }
      setShell({ type: 'ready' })
    }).catch(error => {
      if (!disposed) {
        terminal.options.disableStdin = true
        setShell({ type: 'error', message: error instanceof Error ? error.message : String(error) })
      }
    })

    return () => {
      disposed = true
      observer.disconnect()
      themeObserver.disconnect()
      input.dispose()
      resizing.dispose()
      terminal.dispose()
      instance.current = null
      if (id && !exited) void desktopClient.projectTerminalClose(id).catch(() => {})
    }
  }, [conversationId, attempt])

  useEffect(() => {
    if (active && shell.type === 'ready') instance.current?.focus()
  }, [active, shell.type])

  const stopped = shell.type === 'exited' || shell.type === 'error'
  const status = shell.type === 'starting' ? '正在启动…'
    : shell.type === 'exited' ? shell.code === null ? '已退出' : `已退出 · ${shell.code}`
      : shell.type === 'error' ? '终端不可用' : ''
  return <div className="fox-project-terminal">
    <div className="fox-project-terminal-bar">
      <Terminal aria-hidden="true" />
      <strong>PowerShell</strong>
      <span className="fox-project-terminal-path" title={projectRoot}>{projectRoot.replace(/^[\\/]{2}\?[\\/]UNC[\\/]/iu, '\\\\').replace(/^[\\/]{2}\?[\\/]/u, '')}</span>
      {status && <span className="fox-project-terminal-status" role="status">{status}</span>}
      {stopped && <button type="button" className="fox-panel-icon-button" aria-label="重新打开 PowerShell" title="重新打开 PowerShell" onClick={() => { setShell({ type: 'starting' }); setAttempt(current => current + 1) }}><RotateCcw /></button>}
    </div>
    {shell.type === 'error' && <p className="fox-project-terminal-error" role="alert">{shell.message}</p>}
    <div className="fox-project-terminal-host" ref={host} />
  </div>
}

function terminalTheme(style: CSSStyleDeclaration): ITheme {
  const color = (token: string) => style.getPropertyValue(token).trim()
  const foreground = color('--fox-text')
  const accent = color('--fox-accent')
  const red = color('--fox-danger')
  const green = color('--fox-success')
  const yellow = color('--fox-warning-text')
  return {
    background: color('--fox-card'), foreground, cursor: foreground, selectionBackground: color('--fox-hover'),
    black: foreground, brightBlack: color('--fox-muted'), white: foreground, brightWhite: foreground,
    red, brightRed: red, green, brightGreen: green, yellow, brightYellow: yellow,
    blue: accent, brightBlue: accent, cyan: accent, brightCyan: accent,
  }
}
