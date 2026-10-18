// Package-2-D closeout, R-3: reveal the project popover with a real hover, then
// open the fixture conversation with a real click.

import { evaluate, finish } from './cdp-drive.mjs'

const opened = await evaluate(`(() => {
  const log = []
  // The popover is gated on hover/focus, so a plain click never sees it.
  const toggle = document.querySelector('button[aria-label^="展开项目列表"]')
  if (!toggle) return ['no-project-toggle']
  const entry = toggle.closest('.fox-sidebar-collapsed-entry') || toggle.parentElement
  for (const type of ['mouseenter', 'mouseover', 'focusin']) {
    entry.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }))
  }
  log.push('hovered')
  return log
})()`)
console.log('HOVER ' + JSON.stringify(opened))
await new Promise((resolve) => setTimeout(resolve, 800))

const clicked = await evaluate(`(() => {
  const entries = [...document.querySelectorAll('button[role="menuitem"]')]
  const projectEntry = entries.find((node) => (node.getAttribute('title') || '').includes('g4-p2d-closeout-project'))
  if (!projectEntry) {
    return { clicked: null, available: entries.map((n) => n.getAttribute('title')).slice(0, 10) }
  }
  projectEntry.click()
  return { clicked: projectEntry.getAttribute('title'), label: (projectEntry.textContent || '').trim() }
})()`)
console.log('CLICK ' + JSON.stringify(clicked))
await new Promise((resolve) => setTimeout(resolve, 5000))

const state = await evaluate(`(() => ({
  text: document.body.innerText.slice(0, 2500),
  panel: !!document.querySelector('.fox-delivery-review'),
}))()`)
console.log('AFTER ' + JSON.stringify(state, null, 1))
finish(0)
