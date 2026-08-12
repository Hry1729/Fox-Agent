import React from 'react'
import ReactDOM from 'react-dom/client'
import { App } from './app/App'
import './styles/globals.css'
import { applyStoredTextScale } from './features/settings/text-scale'
import './styles/workbench.css'
import './styles/workspace-pages.css'

document.addEventListener('contextmenu', (event) => event.preventDefault())

applyStoredTextScale()

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
)
