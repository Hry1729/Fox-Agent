import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { cleanup, fireEvent, render } = await import('@testing-library/react')
const { Files, FolderItem, FolderHeader, FolderTrigger, FolderPanel, useFolder, useFiles } = await import('../src/components/animate-ui/primitives/base/files')
afterEach(cleanup)

function FolderState({ name }: { name: string }) {
  return <span data-testid={`${name}-state`}>{String(useFolder().isOpen)}</span>
}
function OpenState() {
  return <output data-testid="open-values">{JSON.stringify(useFiles().open)}</output>
}
function Folder({ name, children }: { name: string; children?: React.ReactNode }) {
  // Keep panels mounted here to assert nested state without relying on a DOM
  // simulator's animation frames. Browser validation covers exit/unmount.
  return <FolderItem value={name}><FolderHeader><FolderTrigger aria-label={name}>{name}<FolderState name={name} /></FolderTrigger></FolderHeader><FolderPanel keepRendered transition={{ duration: 0 }}>{children}</FolderPanel></FolderItem>
}
function NestedFiles({ requests }: { requests: string[][] }) {
  const [open, setOpen] = React.useState(['root'])
  const change = (next: string[]) => { requests.push(next); setOpen(next) }
  return <Files open={open} onOpenChange={change}>
    <OpenState />
    <Folder name="root"><Files open={open} onOpenChange={change}>
      <Folder name="src"><Files open={open} onOpenChange={change}><Folder name="src/deep">deep file</Folder></Files></Folder>
      <Folder name="docs"><Files open={open} onOpenChange={change}><Folder name="docs/guide">guide file</Folder></Files></Folder>
    </Files></Folder>
  </Files>
}

function expanded(view: ReturnType<typeof render>, name: string, expected: boolean) {
  expect(view.getByRole('button', { name, exact: true }).getAttribute('aria-expanded')).toBe(String(expected))
  expect(view.getByTestId(`${name}-state`).textContent).toBe(String(expected))
}

describe('real Base/Motion nested Files', () => {
  test('shared values retain sibling and parent expansion through child and root collapse/reopen', async () => {
    const requests: string[][] = []
    const view = render(<NestedFiles requests={requests} />)
    for (const name of ['src', 'docs', 'src/deep', 'docs/guide']) {
      fireEvent.click(await view.findByRole('button', { name, exact: true }))
    }
    expect(requests).toEqual([
      ['root', 'src'], ['root', 'src', 'docs'],
      ['root', 'src', 'docs', 'src/deep'], ['root', 'src', 'docs', 'src/deep', 'docs/guide'],
    ])
    for (const name of ['root', 'src', 'docs', 'src/deep', 'docs/guide']) expanded(view, name, true)
    fireEvent.click(view.getByRole('button', { name: 'src/deep', exact: true }))
    expect(requests.at(-1)).toEqual(['root', 'src', 'docs', 'docs/guide'])
    expanded(view, 'src/deep', false)
    expanded(view, 'docs/guide', true)
    fireEvent.click(view.getByRole('button', { name: 'docs', exact: true }))
    expect(requests.at(-1)).toEqual(['root', 'src', 'docs/guide'])
    expanded(view, 'src', true)
    expanded(view, 'docs', false)
    fireEvent.click(view.getByRole('button', { name: 'docs', exact: true }))
    expanded(view, 'docs/guide', true)
    fireEvent.click(view.getByRole('button', { name: 'root', exact: true }))
    expect(requests.at(-1)).toEqual(['src', 'docs/guide', 'docs'])
    expanded(view, 'root', false)
    fireEvent.click(view.getByRole('button', { name: 'root', exact: true }))
    expanded(view, 'src', true)
    expanded(view, 'docs/guide', true)
    expect(view.getByTestId('open-values').textContent).toBe(JSON.stringify(['src', 'docs/guide', 'docs', 'root']))
  })

  test('rejecting a controlled folder request keeps trigger, panel and folder context aligned', () => {
    const requests: string[][] = []
    const view = render(<Files open={[]} onOpenChange={next => requests.push(next)}><Folder name="locked">hidden file</Folder></Files>)
    fireEvent.click(view.getByRole('button', { name: 'locked', exact: true }))
    expanded(view, 'locked', false)
    expect(view.getByText('hidden file').hasAttribute('data-closed')).toBe(true)
    expect(requests).toEqual([['locked']])
  })

  test('uncontrolled folders honor defaults and toggle context, panel and trigger together', async () => {
    const requests: string[][] = []
    const view = render(<Files defaultOpen={['initial']} onOpenChange={next => requests.push(next)}><Folder name="initial">visible file</Folder><Folder name="other">other file</Folder></Files>)
    expanded(view, 'initial', true)
    fireEvent.click(view.getByRole('button', { name: 'other', exact: true }))
    expanded(view, 'other', true)
    fireEvent.click(view.getByRole('button', { name: 'initial', exact: true }))
    expanded(view, 'initial', false)
    expect(view.getByText('visible file').hasAttribute('data-closed')).toBe(true)
    expect(requests).toEqual([['initial', 'other'], ['other']])
  })
})
