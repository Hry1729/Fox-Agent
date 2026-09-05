import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { visualizer } from 'rollup-plugin-visualizer'
import { fileURLToPath, URL } from 'node:url'
import { execSync } from 'node:child_process'

const root = fileURLToPath(new URL('.', import.meta.url))

function git(args: string): string {
  return execSync(`git ${args}`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim()
}

/**
 * Whether the tree has real uncommitted source changes. Pure CR-at-EOL
 * (CRLF/LF) differences are ignored so line-ending noise does not flag a
 * build as dirty. Tracked (unstaged), staged and untracked states are checked
 * separately; gitignored build output (dist/node_modules/target) is excluded.
 */
function isDirty(): boolean {
  try {
    const unstaged = git('diff --ignore-cr-at-eol --name-only')
    const staged = git('diff --cached --ignore-cr-at-eol --name-only')
    const untracked = git('ls-files --others --exclude-standard')
    return Boolean(unstaged || staged || untracked)
  } catch {
    return false
  }
}

function gitBuildId(mode: string): string {
  try {
    const sha = git('rev-parse --short HEAD')
    return `${mode}+${sha}${isDirty() ? '-dirty' : ''}`
  } catch {
    return `${mode}+unknown`
  }
}

// `vite build --mode profile` (and `vite --mode profile`) does NOT rely on any
// .env file. The mode drives two static compile-time constants and the
// react-dom alias; a clean checkout therefore gets identical behaviour. The
// profile build swaps react-dom/client for react-dom/profiling so <Profiler
// onRender> fires in an optimized (non-dev) bundle. Normal builds never touch
// this and tree-shake every profiling branch.
export default defineConfig(({ mode }) => {
  const profilingEnabled = mode === 'profile'
  const buildId = gitBuildId(mode)

  return {
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
    define: {
      __FOX_PROFILING__: JSON.stringify(profilingEnabled),
      __FOX_BUILD_ID__: JSON.stringify(buildId),
    },
    resolve: {
      alias: [
        // Package subpath so Vite/pnpm resolve react-dom's internal CJS.
        ...(profilingEnabled
          ? [{ find: /^react-dom\/client$/, replacement: 'react-dom/profiling' }]
          : []),
        { find: '@', replacement: fileURLToPath(new URL('./src', import.meta.url)) },
      ],
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
          // Only force stable vendor/viewer boundaries. Heavy lazily-imported
          // deps (G6, streamdown/mermaid, Shiki) are NOT force-named; doing so
          // earlier promoted them into the entry's static preload graph.
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
  }
})
