import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { StartupLayout } from '../src/app/StartupLayout.tsx'

export const startupLayout = {
  name: 'fox-startup-layout',
  transformIndexHtml: {
    order: 'pre',
    handler(html) {
      return html.replace('<!-- fox-startup-layout -->', () => renderToStaticMarkup(createElement(StartupLayout)))
    },
  },
}
