import React, { Suspense, lazy, useState } from 'react'
import ReactDOM from 'react-dom/client'
import { App } from './app/App'
import './styles/globals.css'
import { applyStoredTextScale } from './features/settings/text-scale'
import './styles/workbench.css'
import './styles/workspace-pages.css'

document.addEventListener('contextmenu', (event) => event.preventDefault())

applyStoredTextScale()

// Static compile-time constant (vite define). False in normal builds, so the
// lazy() calls below are dead-code eliminated and no profiling chunk ships.
const PROFILING = typeof __FOX_PROFILING__ !== 'undefined' && __FOX_PROFILING__ === true

const ProfilingHarness = PROFILING
  ? lazy(() => import('./features/profiling/ProfilingHarness').then((module) => ({ default: module.ProfilingHarness })))
  : null
const RealProfilingController = PROFILING
  ? lazy(() => import('./features/profiling/RealProfilingController').then((module) => ({ default: module.RealProfilingController })))
  : null

function profilingRequested(): boolean {
  try {
    return new URLSearchParams(window.location.search).has('foxPerf')
  } catch {
    return false
  }
}

function openHarness() {
  const url = new URL(window.location.href)
  url.search = 'foxPerf=1'
  window.location.assign(url.toString())
}

function Root() {
  // Mutex: ?foxPerf mounts ONLY the synthetic harness (real App never renders).
  // Otherwise the real App renders plus, in profile builds, the live controller.
  const [harnessMode] = useState<boolean>(() => PROFILING && profilingRequested())

  if (ProfilingHarness && harnessMode) {
    return (
      <Suspense fallback={<div style={{ padding: 24, fontFamily: 'monospace' }}>加载性能夹具…</div>}>
        <ProfilingHarness />
      </Suspense>
    )
  }

  return (
    <>
      <App />
      {RealProfilingController && (
        <Suspense fallback={null}>
          <RealProfilingController />
        </Suspense>
      )}
      {ProfilingHarness && (
        <button
          type="button"
          onClick={openHarness}
          style={{ position: 'fixed', right: 12, bottom: 52, zIndex: 99998, padding: '4px 10px', fontSize: 12, fontFamily: 'monospace', borderRadius: 6, border: '1px solid rgba(128,128,128,0.5)', background: 'rgba(0,0,0,0.7)', color: '#fff', cursor: 'pointer' }}
          title="打开合成性能夹具"
        >
          HARNESS
        </button>
      )}
    </>
  )
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>
)
