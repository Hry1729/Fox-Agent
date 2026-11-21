import { useState } from 'react'
import type { RightMode } from '../fox-shared'

export function useRightPanelNavigation(initialMode: RightMode | null = null, initiallyCollapsed = true) {
  const [mode, setMode] = useState(initialMode)
  const [collapsed, setCollapsed] = useState(initiallyCollapsed)
  const [maximized, setMaximized] = useState(false)

  const openGuide = () => {
    setMode(null)
    setMaximized(false)
    setCollapsed(false)
  }
  const collapse = () => {
    setMode(null)
    setMaximized(false)
    setCollapsed(true)
  }

  return { mode, setMode, collapsed, setCollapsed, maximized, setMaximized, openGuide, collapse }
}
