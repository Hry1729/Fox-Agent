export async function xiaofoxUi(action, args = {}) {
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))
  const visible = el => el && el.getClientRects().length > 0
  const find = (selector, text, root = document) => [...root.querySelectorAll(selector)].find(el => visible(el) && (!text || el.textContent.trim() === text))
  const wait = async (label, get, timeout = 25000) => {
    const end = Date.now() + timeout
    while (Date.now() < end) { const item = get(); if (item) return item; await sleep(100) }
    const controls = [...document.querySelectorAll('button,[role="menuitem"]')].filter(visible)
      .slice(0, 30).map(el => el.getAttribute('aria-label') || el.textContent.trim())
    throw new Error('界面未显示：' + label + '；可见控件：' + JSON.stringify(controls))
  }
  const click = async (label, get) => (await wait(label, get)).click()
  const set = async (label, value) => {
    const element = await wait(label, () => find(`[aria-label="${label}"]`))
    const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, value)
    element.dispatchEvent(new Event('input', { bubbles: true }))
    element.dispatchEvent(new Event('change', { bubbles: true }))
    await sleep(100)
  }
  const button = (text, root = document) => find('button', text, root)
  const entry = await wait('应用入口', () => find('.fox-onboarding-skip') || find('button[aria-label="新建小 Fox"]'))
  const skippedOnboarding = entry.matches('.fox-onboarding-skip')
  if (skippedOnboarding) {
    entry.click()
    await wait('小 Fox 栏', () => find('button[aria-label="新建小 Fox"]'))
  }
  const openContact = async name => {
    await click('联系人 ' + name, () => [...document.querySelectorAll('.fox-contact-link')].find(el => visible(el) && el.querySelector('.fox-contact-row-name')?.textContent === name))
    await wait('私聊标题', () => document.querySelector('.fox-contact-heading[title="' + CSS.escape(name) + '"]'))
    if (!visible(document.querySelector('.fox-contact-side-card'))) await click('私聊资料', () => document.querySelector('.fox-contact-heading'))
  }
  if (action === 'create') {
    await wait('小 Fox 栏', () => find('button[aria-label="新建小 Fox"]'))
    await wait('现有联系人加载', () => document.querySelectorAll('.fox-contact-row-name').length === args.beforeCount)
    const before = [...document.querySelectorAll('.fox-contact-row-name')].map(el => el.textContent)
    const trigger = await wait('新建小 Fox', () => find('button[aria-label="新建小 Fox"]'))
    trigger.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, ctrlKey: false, pointerType: 'mouse' }))
    const item = await wait('新建菜单', () => find('[role="menuitem"]', '新建小 Fox'))
    item.click()
    const heading = await wait('新建后的固定私聊', () => {
      const el = document.querySelector('.fox-contact-heading')
      return visible(el) && [...document.querySelectorAll('.fox-contact-row-name')].length === before.length + 1 && el
    })
    return { skippedOnboarding, beforeCount: before.length, afterCount: document.querySelectorAll('.fox-contact-row-name').length, activeName: heading.title, configurationDialog: !!find('[role="dialog"]') }
  }
  if (action === 'profile') {
    await openContact(args.name)
    await click('设置', () => find('.fox-contact-side-card button[aria-label="设置"]'))
    await set('职责', args.duty)
    await set('沟通偏好', args.preference)
    await click('保存资料', () => button('保存资料'))
    await wait('保存完成', () => find('[role="status"]', '已保存联系人资料') || !document.querySelector('.fox-contact-side-card button:disabled'))
    await click('返回详情', () => find('.fox-contact-side-card button[aria-label="返回详情"]'))
    await click('设置', () => find('.fox-contact-side-card button[aria-label="设置"]'))
    return { duty: document.querySelector('textarea[aria-label="职责"]').value, preference: document.querySelector('textarea[aria-label="沟通偏好"]').value }
  }
  if (action === 'inspect') {
    await openContact(args.name)
    const card = document.querySelector('.fox-contact-side-card')
    return { createButtons: [...document.querySelectorAll('button[aria-label="新建小 Fox"]')].filter(visible).length, sideCards: [...document.querySelectorAll('.fox-contact-side-card')].filter(visible).length, sideText: card.innerText, visibleNavigation: [...document.querySelectorAll('button,a')].filter(visible).map(el => ({ text: el.textContent.trim(), label: el.getAttribute('aria-label') })).filter(x => x.text || x.label) }
  }
  if (action === 'artifact') {
    await openContact(args.name)
    const closeSearch = find('.fox-contact-search-chip button[aria-label="关闭搜索"]')
    if (closeSearch) { closeSearch.click(); await sleep(100) }
    const back = find('.fox-contact-side-card button[aria-label="返回详情"]')
    if (back) { back.click(); await sleep(100) }
    await click('资料库标签', () => button('资料库', document.querySelector('.fox-contact-side-card')))
    await wait('资料库加载', () => document.querySelector('.fox-library') && !find('.fox-library-loading'))
    for (const more of [...document.querySelectorAll('.fox-library-more')].filter(visible)) {
      if (more.getAttribute('aria-expanded') === 'false') more.click()
    }
    const row = await wait('产物文件 ' + args.filename, () => [...document.querySelectorAll('.fox-library-row')].find(el => visible(el) && el.getAttribute('aria-label') === args.filename))
    const rowText = row.innerText
    const menu = async () => {
      const trigger = await wait('文件操作', () => find('.fox-library-menu', null, row))
      trigger.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, ctrlKey: false, pointerType: 'mouse' }))
      return wait('文件菜单', () => find('.fox-library-file-menu'))
    }
    let popup = await menu()
    const menuText = popup.innerText
    const openItem = await wait('打开', () => find('[role="menuitem"]', '打开', popup))
    const jumpItem = await wait('回到消息', () => find('[role="menuitem"]', '回到消息', popup))
    const enabled = el => el.getAttribute('data-disabled') === null && el.getAttribute('aria-disabled') !== 'true'
    const openEnabled = enabled(openItem)
    const jumpEnabled = enabled(jumpItem)
    if (!openEnabled || !jumpEnabled) return { rowText, menuText, openEnabled, jumpEnabled, opened: false, located: false }
    openItem.click()
    await sleep(1000)
    const alertsAfterOpen = [...document.querySelectorAll('[role="alert"]')].filter(visible).map(el => el.innerText)
    popup = await menu()
    await click('回到消息', () => find('[role="menuitem"]', '回到消息', popup))
    const target = await wait('消息定位标记', () => document.querySelector('.is-search-hit[data-turn-message-id], .is-search-hit[data-bubble-id]'), 25000)
    const chip = find('.fox-contact-search-chip')
    return { rowText, menuText, openEnabled, jumpEnabled, opened: true, alertsAfterOpen, located: true, targetText: target.textContent, targetId: target.dataset.turnMessageId || target.dataset.bubbleId || target.id || null, locationStatus: chip?.innerText ?? null }
  }
  if (action === 'expert') {
    if (args.conversationTitle) {
      await click('普通对话', () => [...document.querySelectorAll('button')].find(el => visible(el) && el.textContent.trim() === args.conversationTitle))
    } else {
      await click('新建对话', () => find('button[aria-label="新建对话"]'))
    }
    const chip = () => find('button.fox-expert-chip')
    const openPicker = async () => {
      const trigger = await wait('专家选择入口', chip)
      if (trigger.title.startsWith('选择专家')) trigger.click()
      else {
        trigger.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, button: 0, ctrlKey: false, pointerType: 'mouse' }))
        await click('更换专家', () => find('[role="menuitem"]', '更换专家'))
      }
      await wait('专家选择窗口', () => find('.fox-expert-picker-dialog'))
    }
    const choose = async name => {
      await openPicker()
      const option = await wait('专家 ' + name, () => [...document.querySelectorAll('.fox-expert-picker-dialog [role="button"]')].find(el => visible(el) && el.getAttribute('aria-label') === `与${name}开始对话` && el.getAttribute('aria-disabled') !== 'true'))
      option.click()
      await wait('专家窗口关闭', () => !visible(document.querySelector('.fox-expert-picker-dialog')))
      return (await wait('专家控件更新', () => chip()?.title.includes(name) && chip())).title
    }
    const selected = await choose(args.firstExpert)
    const changed = await choose(args.secondExpert)
    await choose(args.defaultExpert)
    await wait('移除完成', () => !chip() || chip().title !== changed)
    return { selected, changed, afterRemove: chip()?.title ?? null, removalPath: '选择默认专家解除绑定' }
  }
  if (action === 'task' || action === 'task_status') {
    await openContact(args.name)
    const back = find('.fox-contact-side-card button[aria-label="返回详情"]')
    if (back) { back.click(); await sleep(100) }
    await click('详情标签', () => button('详情', document.querySelector('.fox-contact-side-card')))
    await click('定时任务', () => find('.fox-contact-side-item', '定时任务暂无任务') || [...document.querySelectorAll('.fox-contact-side-item')].find(el => visible(el) && el.textContent.includes('定时任务')))
    if (action === 'task_status') {
      const card = await wait('任务状态', () => [...document.querySelectorAll('.fox-contact-task')].find(el => el.getAttribute('aria-label') === args.taskName && (!args.expectedStatus || el.innerText.includes(args.expectedStatus))))
      return { text: card.innerText }
    }
    await click('添加任务', () => button('添加任务'))
    await set('任务名称', args.taskName)
    await set('执行要求', args.objective)
    await set('间隔数值', '24')
    await click('保存任务', () => button('保存', document.querySelector('.fox-contact-task-editor')))
    let card = await wait('已创建任务', () => [...document.querySelectorAll('.fox-contact-task')].find(el => el.getAttribute('aria-label') === args.taskName))
    const created = card.innerText
    await click('编辑任务', () => button('编辑', card))
    await set('执行要求', args.editedObjective)
    await click('保存修改', () => button('保存', document.querySelector('.fox-contact-task-editor')))
    card = await wait('修改后的任务', () => [...document.querySelectorAll('.fox-contact-task')].find(el => el.getAttribute('aria-label') === args.taskName && el.textContent.includes(args.editedObjective)))
    const edited = card.innerText
    await click('立即执行', () => button('立即执行', card))
    await wait('运行结果', () => {
      card = [...document.querySelectorAll('.fox-contact-task')].find(el => el.getAttribute('aria-label') === args.taskName)
      return card && /最近结果：(?:待派发|排队中|执行中|已完成|失败|已取消|已拒绝|已跳过)/.test(card.innerText)
    }, 60000)
    await click('运行记录', () => button('运行记录', card))
    const history = await wait('运行记录', () => card.querySelector('.fox-contact-task-runs'))
    return { created, edited, afterRun: card.innerText, history: history.innerText, historyControls: [...history.querySelectorAll('button,a')].map(el => ({ text: el.textContent.trim(), href: el.getAttribute('href'), label: el.getAttribute('aria-label') })) }
  }
  throw new Error('未知 UI 验收动作：' + action)
}
