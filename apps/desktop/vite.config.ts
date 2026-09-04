import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { visualizer } from 'rollup-plugin-visualizer'
import { fileURLToPath, URL } from 'node:url'

const root = fileURLToPath(new URL('.', import.meta.url))

export default defineConfig({
  root,
  plugins: [
    react(),
    tailwindcss(),
    visualizer({
      filename: 'dist/bundle-stats.html',
      template: 'network',
      gzipSize: true,
      brotliSize: true,
      open: false,
    }),
  ],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  optimizeDeps: {
    exclude: ['motion', 'motion/react'],
  },
  worker: {
    format: 'es'
  },
  build: {
    rollupOptions: {
      output: {
        // Only force stable vendor/viewer boundaries. Heavy lazily-imported deps
        // (G6, streamdown/mermaid, Shiki) are NOT force-named: doing so earlier
        // promoted them into the entry's static preload graph. Letting Rollup
        // follow the natural dynamic-import graph keeps them in on-demand chunks.
        manualChunks(id) {
          if (!id.includes('node_modules')) return undefined
          if (/[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(id)) return 'react-vendor'
          if (id.includes('pdfjs-dist')) return 'viewer-pdf'
          if (id.includes('docx-preview')) return 'viewer-docx'
          if (id.includes('@js-preview/excel')) return 'viewer-excel'
          if (id.includes('@aiden0z/pptx-renderer')) return 'viewer-pptx'
          if (id.includes('@xyflow/react')) return 'graph-flow'
          if (id.includes('motion')) return 'motion-vendor'
          if (id.includes('lucide-react') || id.includes('@remixicon/react')) return 'icons-vendor'
          return undefined
        },
      },
    },
  },
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: 1421,
    strictPort: true
  }
})
