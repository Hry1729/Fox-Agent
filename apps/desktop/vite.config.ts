import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath, URL } from 'node:url'

const root = fileURLToPath(new URL('.', import.meta.url))

export default defineConfig({
  root,
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) }
  },
  optimizeDeps: {
    exclude: ['motion', 'motion/react']
  },
  worker: {
    format: 'es'
  },
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: 1421,
    strictPort: true
  }
})
