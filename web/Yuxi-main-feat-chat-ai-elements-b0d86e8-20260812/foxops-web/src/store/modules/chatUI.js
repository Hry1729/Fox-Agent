import { defineStore } from 'pinia'
import { ref } from 'vue'

/**
 * 对话界面 UI 状态（搬自 Yuxi stores/chatUI.js）
 * 注意：sidebarCollapsed 在 Art Design Pro 布局下可能由模板自管，这里保留供对话组件按需用。
 */
export const useChatUIStore = defineStore('chatUI', () => {
  // 加载状态
  const isLoadingMessages = ref(false)

  // 应用侧边栏折叠态
  const sidebarCollapsed = ref(false)

  // 更多菜单
  const moreMenuOpen = ref(false)
  const moreMenuPosition = ref({ x: 0, y: 0 })

  /** 打开更多菜单 */
  function openMoreMenu(x, y) {
    moreMenuPosition.value = { x, y }
    moreMenuOpen.value = true
  }

  /** 关闭更多菜单 */
  function closeMoreMenu() {
    moreMenuOpen.value = false
  }

  /** 重置所有 UI 状态（不包括持久化状态） */
  function reset() {
    isLoadingMessages.value = false
    moreMenuOpen.value = false
    moreMenuPosition.value = { x: 0, y: 0 }
  }

  return {
    isLoadingMessages,
    sidebarCollapsed,
    moreMenuOpen,
    moreMenuPosition,
    openMoreMenu,
    closeMoreMenu,
    reset
  }
})
