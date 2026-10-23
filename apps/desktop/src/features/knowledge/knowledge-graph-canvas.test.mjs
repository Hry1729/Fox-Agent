import { afterEach, expect, mock, test } from 'bun:test'
import { createElement } from 'react'
import { cleanup, render, waitFor } from '@testing-library/react'

const graphs = []
class FakeGraph {
  rendered = false
  zoom = 0.75
  renders = 0
  draws = 0
  handlers = new Map()
  constructor(options) { this.options = options; graphs.push(this) }
  on(event, callback) { this.handlers.set(event, callback) }
  async render() {
    this.renders++
    this.handlers.get('aftertransform')?.()
    this.rendered = true
  }
  getZoom() { if (!this.rendered) throw new Error('viewport not initialized'); return this.zoom }
  async fitView() {}
  setData(data) { this.data = data }
  setNode(node) { this.options.node = node }
  async draw() { this.draws++ }
  async setElementState(state) { this.states = state }
  async frontElement() {}
  destroy() {}
}
mock.module('@antv/g6', () => ({ Graph: FakeGraph }))
const { KnowledgeGraphCanvas } = await import('./knowledge-graph-canvas')
afterEach(() => { cleanup(); graphs.length = 0 })

test('initial transform waits for the viewport; changing shape preserves layout, zoom and selection', async () => {
  const nodes = [{ id: 'a', label: '这是需要截断的很长的中文资料名称', type: '文档', properties: {} }, { id: 'b', label: '关联资料', type: '概念', properties: {} }]
  const edges = [{ id: 'ab', source: 'a', target: 'b', label: '引用', properties: {} }]
  const zooms = []
  const props = { nodes, edges, selectedId: 'a', onSelect() {}, onExpand() {}, onZoomChange: zoom => zooms.push(zoom) }
  const view = render(createElement(KnowledgeGraphCanvas, props))
  await waitFor(() => expect(graphs[0]?.states?.a).toEqual(['selected']))
  const graph = graphs[0]
  expect(zooms).toContain(0.75)
  const renders = graph.renders
  view.rerender(createElement(KnowledgeGraphCanvas, { ...props, nodeShape: 'circle' }))
  await waitFor(() => expect(graph.options.node.type).toBe('circle'))
  await waitFor(() => expect(graph.draws).toBe(1))
  expect(graph.renders).toBe(renders)
  expect(graph.zoom).toBe(0.75)
  expect(graph.states.a).toEqual(['selected'])
  expect(graph.states.b).toEqual(['neighbor'])
  expect(graph.options.node.style.labelMaxLines).toBe(2)
  view.rerender(createElement(KnowledgeGraphCanvas, { ...props, nodeShape: 'rect' }))
  await waitFor(() => expect(graph.options.node.type).toBe('rect'))
  expect(graph.renders).toBe(renders)
  expect(graph.options.node.style.labelMaxLines).toBe(1)
})
