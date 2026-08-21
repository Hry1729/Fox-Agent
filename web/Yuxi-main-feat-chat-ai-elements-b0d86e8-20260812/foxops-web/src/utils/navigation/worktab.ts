/**
 * 工作标签页管理模块
 *
 * 提供工作标签页（Worktab）的自动管理功能
 *
 * ## 主要功能
 *
 * - 根据路由导航自动创建和更新工作标签页
 * - iframe 页面标签页特殊处理
 * - 标签页信息提取（标题、路径、缓存状态等）
 * - 固定标签页支持
 * - 根据系统设置控制标签页显示
 * - 首页标签页特殊处理
 *
 * ## 使用场景
 *
 * - 路由守卫中自动创建标签页
 * - 页面切换时更新标签页状态
 * - 多标签页导航系统
 *
 * @module utils/navigation/worktab
 * @author Art Design Pro Team
 */
import { useWorktabStore } from '@/store/modules/worktab'
import { RouteLocationNormalized } from 'vue-router'
import { isIframe } from './route'
import { useSettingStore } from '@/store/modules/setting'
import { IframeRouteManager } from '@/router/core'
import { useCommon } from '@/hooks/core/useCommon'
import { router } from '@/router'
import { WorkTab } from '@/types'

/**
 * 查找与隐藏详情页关联的标签页索引
 */
export const findRelatedTabIndex = (
  opened: WorkTab[],
  activePath: string,
  routeName?: string
): number => {
  const byListPath = opened.findIndex((tab) => tab.path === activePath)
  if (byListPath >= 0) return byListPath

  const byParentPath = opened.findIndex((tab) => tab.parentPath === activePath)
  if (byParentPath >= 0) return byParentPath

  if (routeName) {
    const byName = opened.findIndex((tab) => tab.name === routeName)
    if (byName >= 0) return byName
  }

  return -1
}

/**
 * 解析列表页路由元信息，供直接打开详情页时创建标签
 */
const resolveParentTabMeta = (activePath: string): Partial<WorkTab> => {
  const resolved = router.resolve({ path: activePath })
  const matched = resolved.matched[resolved.matched.length - 1]

  return {
    title: String(matched?.meta?.title || ''),
    icon: matched?.meta?.icon as string | undefined,
    name: String(matched?.name || ''),
    keepAlive: Boolean(matched?.meta?.keepAlive)
  }
}

/**
 * 隐藏详情页：复用列表标签并更新为当前详情路径
 */
const setHiddenDetailWorktab = (to: RouteLocationNormalized): void => {
  const worktabStore = useWorktabStore()
  const { meta, path, name, params, query } = to
  const activePath = String(meta.activePath || '')

  if (!activePath) return

  const settingStore = useSettingStore()
  const { homePath } = useCommon()
  if (!settingStore.showWorkTab && path !== homePath.value) {
    return
  }

  const relatedIndex = findRelatedTabIndex(worktabStore.opened, activePath, name as string)
  const existingTab = relatedIndex >= 0 ? worktabStore.opened[relatedIndex] : undefined
  const parentMeta = resolveParentTabMeta(activePath)

  worktabStore.openTab({
    title: existingTab?.title || parentMeta.title || String(meta.title || ''),
    icon: existingTab?.icon || parentMeta.icon || (meta.icon as string),
    path,
    name: name as string,
    keepAlive: (meta.keepAlive as boolean) ?? existingTab?.keepAlive ?? parentMeta.keepAlive ?? false,
    params,
    query,
    fixedTab: existingTab?.fixedTab,
    parentPath: activePath,
    customTitle: existingTab?.customTitle
  })
}

/**
 * 根据当前路由信息设置工作标签页（worktab）
 * @param to 当前路由对象
 */
export const setWorktab = (to: RouteLocationNormalized): void => {
  const worktabStore = useWorktabStore()
  const { meta, path, name, params, query } = to

  // 隐藏详情页：更新关联列表标签的路径，避免标签失焦且切回时落到列表页
  if (meta.isHideTab && meta.activePath) {
    setHiddenDetailWorktab(to)
    return
  }

  if (!meta.isHideTab) {
    // 如果是 iframe 页面，则特殊处理工作标签页
    if (isIframe(path)) {
      const iframeRoute = IframeRouteManager.getInstance().findByPath(to.path)

      if (iframeRoute?.meta) {
        worktabStore.openTab({
          title: iframeRoute.meta.title,
          icon: meta.icon as string,
          path,
          name: name as string,
          keepAlive: meta.keepAlive as boolean,
          params,
          query,
          parentPath: ''
        })
      }
    } else if (useSettingStore().showWorkTab || path === useCommon().homePath.value) {
      worktabStore.openTab({
        title: meta.title as string,
        icon: meta.icon as string,
        path,
        name: name as string,
        keepAlive: meta.keepAlive as boolean,
        params,
        query,
        fixedTab: meta.fixedTab as boolean,
        parentPath: ''
      })
    }
  }
}
