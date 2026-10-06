import { expect, test, spyOn } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { useProjectCatalog } from '../src/features/conversations/hooks/use-project-catalog'
import { desktopClient } from '../src/features/conversations/api/desktop-client'
import type { ProjectRecord } from '../src/features/conversations/model/types'

if (typeof document === 'undefined') GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const project: ProjectRecord = { id:'project',name:'Project',rootPath:'D:/project',permissionMode:'ask',status:'active',createdAt:1,updatedAt:1,lastOpenedAt:null,pinned:false }

test('an older project list response cannot undo a successful pin', async () => {
  let release!: (items:ProjectRecord[]) => void
  const delayed = new Promise<ProjectRecord[]>(resolve => { release = resolve })
  const list = spyOn(desktopClient,'listProjects').mockReturnValue(delayed)
  const pin = spyOn(desktopClient,'setProjectPinned').mockResolvedValue({...project,pinned:true})
  let catalog!: ReturnType<typeof useProjectCatalog>
  function Probe(){catalog=useProjectCatalog(true);return null}
  const host=document.createElement('div'),root=createRoot(host)
  try {
    await act(async()=>{root.render(<Probe/>)})
    await act(async()=>{await catalog.setProjectPinned(project.id,true)})
    expect(catalog.projects[0].pinned).toBe(true)
    await act(async()=>{release([project]);await delayed})
    expect(catalog.projects[0].pinned).toBe(true)
    expect(pin).toHaveBeenCalledWith(project.id,true)
  } finally {await act(async()=>root.unmount());list.mockRestore();pin.mockRestore()}
})

test('a rejected pin request does not change the displayed project state', async () => {
  const list=spyOn(desktopClient,'listProjects').mockResolvedValue([project])
  const pin=spyOn(desktopClient,'setProjectPinned').mockRejectedValue(new Error('write failed'))
  let catalog!:ReturnType<typeof useProjectCatalog>
  function Probe(){catalog=useProjectCatalog(true);return null}
  const root=createRoot(document.createElement('div'))
  try {
    await act(async()=>{root.render(<Probe/>);await Promise.resolve()})
    await act(async()=>{await expect(catalog.setProjectPinned(project.id,true)).rejects.toThrow('write failed')})
    expect(catalog.projects[0].pinned).toBe(false)
  } finally {await act(async()=>root.unmount());list.mockRestore();pin.mockRestore()}
})
