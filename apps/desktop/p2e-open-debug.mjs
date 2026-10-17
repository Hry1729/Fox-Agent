// Package-2-E closeout U-1: debug the project list and confirm which project
// holds the target conversation.
import { evaluate, evaluateAsync, finish } from './cdp-drive.mjs'

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

const needsSkip = await evaluate(
  `[...document.querySelectorAll('button')].some((b) => (b.textContent || '').includes('跳过设置'))`,
)
if (needsSkip) {
  const { clickByText } = await import('./cdp-drive.mjs')
  await clickByText('button', '跳过设置')
  await wait(3000)
}

await evaluate(`(() => {
  const toggle = document.querySelector('button[aria-label^="展开项目列表"]')
  if (!toggle) return 'no-toggle'
  const entry = toggle.closest('.fox-sidebar-collapsed-entry') || toggle.parentElement
  for (const type of ['mouseenter', 'mouseover', 'focusin']) {
    entry.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }))
  }
  return 'hovered'
})()`)
await wait(1200)

const projects = await evaluate(`(() => {
  return [...document.querySelectorAll('.fox-sidebar-hover-project')].map((wrap) => {
    const button = wrap.querySelector('button[role="menuitem"]')
    return {
      wrapTitle: wrap.getAttribute('title'),
      buttonTitle: button ? button.getAttribute('title') : null,
      text: wrap.textContent.trim().slice(0, 60),
    }
  })
})()`)
console.log('PROJECTS ' + JSON.stringify(projects, null, 1))

const list = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  return ((await invoke('conversations_list', {}))?.data ?? []).map((c) => ({
    id: c.id, title: c.title, projectRoot: c.projectRoot, lastRunStatus: c.lastRunStatus,
  }))
})()`)
console.log('CONVERSATIONS ' + JSON.stringify(list, null, 1))
finish(0)
