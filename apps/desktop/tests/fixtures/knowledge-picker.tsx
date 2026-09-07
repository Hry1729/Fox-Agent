// Browser-only component fixture. Not an application route or production entry.
import { useState, type ComponentProps } from 'react'
import { createRoot } from 'react-dom/client'
import { KnowledgeBindingDialog } from '../../src/features/chat/workbench'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'

type Props = ComponentProps<typeof KnowledgeBindingDialog>
const bindings: Props['bindings'] = []
const localItems = [
  { id: 'ceshi', name: 'ceshi', description: '', documentCount: 29, activeIndexGeneration: 'generation-1', activeJobStatus: null },
  { id: 'guide', name: 'Fox 使用指南', description: 'Fox Agent 页面与常用操作教程', documentCount: 0, activeIndexGeneration: null, activeJobStatus: null },
] as Props['localItems']
const remoteItems = [{ id: 'remote-demo', name: '远程测试知识库', description: '用于验证跨来源选择', fileCount: 3, status: null }] as Props['remoteItems']
const initial: NonNullable<Props['references']> = [{ source: 'local', providerKey: 'local', id: 'ceshi' }]

function Fixture() {
  const [open, setOpen] = useState(true)
  const [references, setReferences] = useState(initial)
  const [offline, setOffline] = useState(true)
  return <><button onClick={() => setOpen(true)}>重新打开</button><output>已保存 {references.length} 个</output>
    <KnowledgeBindingDialog open={open} onOpenChange={setOpen} remoteItems={offline ? [] : remoteItems} localItems={localItems} localLoading={false} remoteLoading={false} remoteError={offline ? '无法连接知识库，请检查服务连接。' : null} onRetry={() => setOffline(false)} bindings={bindings} references={references} busy={false} onConfirm={(next) => { setReferences(next); setOpen(false) }} />
  </>
}
createRoot(document.getElementById('root')!).render(<Fixture />)
