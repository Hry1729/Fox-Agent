/**
 * 离线图标加载器
 *
 * 预加载 Remix Icon 图标集，避免运行时从 Iconify CDN 拉取（内网/离线会失败）。
 */
import { addCollection } from '@iconify/vue'
import riIcons from '@iconify-json/ri/icons.json'

addCollection(riIcons as Parameters<typeof addCollection>[0])
