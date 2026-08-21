import MarkdownIt from 'markdown-it'
import hljs from 'highlight.js'
import markdownItKatex from '@vscode/markdown-it-katex'
import DOMPurify from 'dompurify'

/**
 * 简化版 Markdown 渲染（搬自 Yuxi markdown_preview.js，去掉 shiki/js-yaml/svg/task-lists）
 * markdown-it + highlight.js 代码高亮 + katex 公式 + dompurify 防 XSS
 *
 * 代码高亮主题跟随 Art Design Pro 主题（亮色/暗色），由 ensureHighlightTheme() 管理。
 */

const md = new MarkdownIt({
  html: true,
  breaks: true,
  linkify: true,
  highlight(code, lang) {
    const language = lang && hljs.getLanguage(lang) ? lang : 'plaintext'
    try {
      return `<pre class="hljs"><code>${hljs.highlight(code, { language }).value}</code></pre>`
    } catch {
      return `<pre class="hljs"><code>${md.utils.escapeHtml(code)}</code></pre>`
    }
  }
}).use(markdownItKatex, { throwOnError: false, errorColor: '#cc0000' })

// 代码高亮 CSS 文件的 Vite 资源 URL（?url 让 Vite 把 CSS 当静态资源输出）
import hljsLight from 'highlight.js/styles/github.css?url'
import hljsDark from 'highlight.js/styles/github-dark.css?url'

/**
 * 让高亮主题跟随 Art Design Pro 主题。
 * 检测 <html> 上是否有 class="dark"（Art Design Pro 的暗色模式标记），
 * 然后创建/更新一个 <link> 元素切换 highlight.js CSS 文件。
 *
 * 在任何地方都能调用，幂等——调多了也只是更新同一个 link.href。
 */
export function ensureHighlightTheme() {
  if (typeof document === 'undefined') return
  const isDark = document.documentElement.classList.contains('dark')
  const expected = isDark ? hljsDark : hljsLight
  let link = document.getElementById('hljs-theme-link')
  if (!link) {
    link = document.createElement('link')
    link.id = 'hljs-theme-link'
    link.rel = 'stylesheet'
    document.head.appendChild(link)
  }
  // 只在 href 变化时才设置，避免无谓的样式重刷
  if (link.getAttribute('href') !== expected) {
    link.setAttribute('href', expected)
  }
}

// 初始化：页面加载时立即跟随当前主题
ensureHighlightTheme()

// 监听 <html> class 变化，自动切换高亮主题（对应 Art Design Pro 的亮色/暗色切换）
if (typeof document !== 'undefined') {
  const observer = new MutationObserver(() => ensureHighlightTheme())
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] })
}

/** 把 markdown 文本渲染成安全的 HTML */
export function renderMarkdown(content) {
  if (!content) return ''
  // 去掉开头/结尾多余换行，避免渲染出开头空白行（AI 回复常以 \n\n 开头）
  const trimmed = String(content).replace(/^\n+/, '').replace(/\n+$/, '')
  const html = md.render(trimmed)
  return DOMPurify.sanitize(html, { ADD_ATTR: ['target', 'rel', 'class'] })
}

export default md
