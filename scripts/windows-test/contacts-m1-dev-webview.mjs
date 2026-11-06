// Isolated Vite acceptance entry. Nothing in the desktop build imports this file.
import { resolve, join } from 'node:path'
import { pathToFileURL } from 'node:url'

const desktop = process.cwd()
const {createServer} = await import(pathToFileURL(resolve(desktop,'node_modules/vite/dist/node/index.js')))
const expected = join(process.env.FOX_DATA_DIR,'companions').replaceAll('\\','/').toLowerCase() + '/'
const moduleName = 'virtual:fox-contact-m1-acceptance'
let client
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
            try { const value = await ipc.invoke(command,args); hot.send('contacts:reply',{id,value}) }
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
  server:{host:'127.0.0.1',port:Number(process.env.FOX_CONTACTS_DEV_PORT || 1421),strictPort:true}})
await server.listen()
