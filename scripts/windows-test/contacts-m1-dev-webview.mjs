// Isolated Vite acceptance entry. Nothing in the desktop build imports this file.
import { resolve, join } from 'node:path'
import { pathToFileURL } from 'node:url'

const desktop = process.cwd()
const {createServer} = await import(pathToFileURL(resolve(desktop,'node_modules/vite/dist/node/index.js')))
const expected = join(process.env.FOX_DATA_DIR,'companions').replaceAll('\\','/').toLowerCase() + '/'
const moduleName = 'virtual:fox-contact-m1-acceptance'
let client

async function openContactNotes(displayName) {
  const until = Date.now() + 25_000
  async function visible(label, select) {
    while (Date.now() < until) {
      const element = select()
      if (element?.getClientRects().length) return element
      await new Promise(done => setTimeout(done, 100))
    }
    throw new Error('联系人界面未显示' + label)
  }
  const contact = () => Array.from(document.querySelectorAll('.fox-contact-link'))
    .find(button => button.querySelector('span')?.textContent === displayName)
  const entry = await visible('小狐或引导入口', () => contact() || document.querySelector('.fox-onboarding-skip'))
  if (entry.matches('.fox-onboarding-skip')) entry.click()
  const link = await visible('联系人', contact)
  link.click()
  const profile = await visible('私聊资料按钮', () => document.querySelector('.fox-chat-topbar [aria-label="联系人资料"]'))
  profile.click()
  const panel = await visible('资料面板', () => document.querySelector('.fox-contact-panel form'))
  const name = panel.querySelector('input').value
  const workspace = panel.parentElement.querySelector('code').textContent
  const notes = await visible('笔记入口', () => Array.from(document.querySelectorAll('[aria-label="资料分类"] button'))
    .find(button => button.textContent.trim() === '笔记'))
  notes.click()
  const title = await visible('主笔记用量', () => Array.from(document.querySelectorAll('.fox-contact-panel h3'))
    .find(heading => heading.textContent.includes('主笔记')))
  return { name, workspace, usage: title.textContent.trim() }
}

const bridge = {
  name:'fox-contacts-isolated-acceptance',
  apply:'serve',
  resolveId(id) {if(id === moduleName) return '\0' + id},
  load(id) {
    if(id !== '\0' + moduleName) return
    return `const hot = import.meta.hot
      const ipc = window.__TAURI_INTERNALS__
      if (hot && ipc) {
        const identity = await ipc.invoke('companions_list')
        const own = identity?.data?.companions?.find(c => c.isDefault)
        if (own?.workspaceDir?.replaceAll('\\\\','/').toLowerCase().startsWith(${JSON.stringify(expected)})) {
          hot.on('contacts:invoke',async ({id,command,args}) => {
            try {
              const value = command === 'acceptance.open_contact_notes'
                ? {ok:true, data:await (${openContactNotes.toString()})(args.displayName)}
                : await ipc.invoke(command,args)
              hot.send('contacts:reply',{id,value})
            }
            catch(error) {hot.send('contacts:reply',{id,error:String(error)})}
          })
          hot.send('contacts:ready',{workspace:own.workspaceDir})
        }
      }`
  },
  transformIndexHtml() {
    return [{tag:'script',attrs:{type:'module',src:'/@id/' + moduleName},injectTo:'body'}]
  },
  configureServer(server) {
    server.ws.on('contacts:ready',(payload,peer) => {
      if(payload.workspace?.replaceAll('\\','/').toLowerCase().startsWith(expected)) {
        client = peer
        process.send({ready:true})
      }
    })
    server.ws.on('contacts:reply',(payload,peer) => { if (client === peer) process.send(payload) })
    process.on('message',request => client?.send('contacts:invoke',request))
  }
}
const server = await createServer({configFile:resolve(desktop,'vite.config.ts'),plugins:[bridge],
  optimizeDeps:{entries:['index.html']},
  server:{host:'127.0.0.1',port:Number(process.env.FOX_CONTACTS_DEV_PORT || 1421),strictPort:true}})
await server.listen()
