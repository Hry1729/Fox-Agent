import { h } from 'vue'

export const MENTION_ICON_SIZE = 15
export const MENTION_ICON_STROKE_WIDTH = 2

// 离线 SVG 图标（替代 lucide-vue-next，避免 CDN 依赖）
const createIcon = (paths) => ({
  name: 'MentionIcon',
  props: { size: { type: Number, default: 15 }, strokeWidth: { type: Number, default: 2 } },
  render() {
    return h('svg', {
      width: this.size, height: this.size, viewBox: '0 0 24 24',
      fill: 'none', stroke: 'currentColor', 'stroke-width': this.strokeWidth,
      'stroke-linecap': 'round', 'stroke-linejoin': 'round'
    }, paths.map((d) => h('path', { d })))
  }
})

const MENTION_TYPE_ICON_COMPONENTS = {
  knowledge: createIcon(['M12 7v14', 'M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z']),
  skill: createIcon(['m19 21-7-4-7 4V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v16z', 'M9 10h6', 'M9 14h4']),
  mcp: createIcon(['M9 2v6', 'M15 2v6', 'M9 8a3 3 0 0 0 6 0', 'M6 14h12v8H6z']),
  subagent: createIcon(['M12 8V4H8', 'M4 8h16', 'M2 12h20', 'M12 22v-6', 'M8 22l4-4 4 4', 'round'])
}

export const getMentionIconComponent = (type) => MENTION_TYPE_ICON_COMPONENTS[type] || MENTION_TYPE_ICON_COMPONENTS.mcp

export const getMentionIconStyle = () => null
