/**
 * vue-stream-markdown 本地 shim。
 * 网络安装 mermaid 依赖不稳定时，先用精简 Markdown 渲染器顶上，
 * 保证 AI Elements message/reasoning 可编译；后续可替换为官方包。
 */
import { defineComponent, h, computed } from 'vue'

export const Markdown = defineComponent({
  name: 'StreamMarkdownShim',
  props: {
    content: { type: String, default: '' },
    class: { type: String, default: '' }
  },
  setup(props) {
    const html = computed(() => props.content || '')
    return () =>
      h('div', {
        class: ['markdown-body', props.class],
        innerHTML: html.value
      })
  }
})

export default Markdown
