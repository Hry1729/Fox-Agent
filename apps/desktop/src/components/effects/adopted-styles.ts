/**
 * Re-publish effect libraries' stylesheets that the host CSP dropped.
 *
 * Tauri puts a nonce into `style-src`, and per CSP3 a nonce makes `'unsafe-inline'`
 * ineffective. Effect libraries that inject their own `<style>` element therefore
 * lose their rules: the element stays in the DOM but never reaches
 * `document.styleSheets` (border-beam's keyframes and metal-fx's layout sheet are
 * both dropped this way — without the latter `.metal-fx-root` has no
 * `position: relative`, so its absolute canvas and glow escape the button).
 *
 * A constructable stylesheet is CSSOM rather than inline style, so CSP does not
 * police it: the exact same text can be re-published as an adopted sheet. The
 * marking is idempotent, so if the CSP is ever relaxed to let the element apply,
 * `isApplied` skips it and no rule is applied twice.
 */
const ADOPTED_ATTRIBUTE = 'cspAdopted'

function isApplied(element: HTMLStyleElement): boolean {
  for (const sheet of Array.from(document.styleSheets)) {
    if (sheet.ownerNode === element) return true
  }
  return false
}

function adoptBlockedSheets(): void {
  if (typeof CSSStyleSheet !== 'function' || !('adoptedStyleSheets' in document)) return
  for (const element of Array.from(document.querySelectorAll('style'))) {
    if (element.dataset[ADOPTED_ATTRIBUTE] === '1' || isApplied(element)) continue
    const text = element.textContent ?? ''
    if (!text.trim()) continue
    try {
      const sheet = new CSSStyleSheet()
      sheet.replaceSync(text)
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet]
      element.dataset[ADOPTED_ATTRIBUTE] = '1'
    } catch {
      // Text this engine cannot parse is the library's business, not ours.
    }
  }
}

/** Adopt what is already blocked, and watch for later injections. */
export function adoptBlockedStyles(): () => void {
  adoptBlockedSheets()
  // A library typically appends an empty <style> first and fills in the text
  // afterwards, so watch both "a style element appeared" and "an existing style
  // element's content changed" (setting `textContent` is a childList mutation on
  // the element itself, which is why `characterData` is not needed here).
  const observer = new MutationObserver((records) => {
    for (const record of records) {
      if (record.target.nodeName === 'STYLE') {
        adoptBlockedSheets()
        return
      }
      for (const node of Array.from(record.addedNodes)) {
        if (node.nodeName === 'STYLE') {
          adoptBlockedSheets()
          return
        }
      }
    }
  })
  observer.observe(document.documentElement, { childList: true, subtree: true })
  return () => observer.disconnect()
}
