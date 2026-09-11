import { defineConfig, mergeConfig } from 'vite'
import base from './vite.config'

// Isolated design-capture preview. Existing development server is unaffected.
export default defineConfig(async (env) => {
  const config = typeof base === 'function' ? await base(env) : await base
  return mergeConfig(config, {
    server: { host: '127.0.0.1', port: 1425, strictPort: true, hmr: false, watch: { ignored: ['**/src-tauri/**'] } },
  })
})
