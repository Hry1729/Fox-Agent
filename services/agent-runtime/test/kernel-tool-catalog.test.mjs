import test from 'node:test'
import assert from 'node:assert/strict'
import { RUNTIME_TOOL_CATALOG } from '../../../packages/fox-engine-protocol/index.mjs'
import { kernelToolDefinitions, validateKernelToolCatalog } from '../src/pi-kernel-loop.mjs'

const definition = name => ({ name, description: `Registered tool ${name}`, parameters: { type: 'object', properties: {} } })

test('tool catalog capacity follows registered schemas rather than 128 items', () => {
  const registered = new Set(Array.from({ length: 160 }, (_, i) => `registry_tool_${i}`))
  const definitions = [...registered].map(definition)
  assert.equal(validateKernelToolCatalog(definitions, registered).size, 160)
  assert.throws(() => validateKernelToolCatalog([...definitions, definitions[0]], registered), /schema/)
  assert.throws(() => validateKernelToolCatalog([...definitions, definition('unknown')], registered), /schema/)
  assert.throws(() => validateKernelToolCatalog([{ ...definitions[0], parameters: { type: 'array' } }], registered), /schema/)
  assert.throws(() => validateKernelToolCatalog([{ ...definitions[0], parameters: { type: 'object', description: 'x'.repeat(131_073) } }], registered), /too large/)
})

test('production catalog still admits only the frozen Fox registry', () => {
  const definitions = RUNTIME_TOOL_CATALOG.map(tool => definition(tool.name))
  assert.equal(kernelToolDefinitions(definitions).size, definitions.length)
  assert.throws(() => kernelToolDefinitions([definition('registry_tool_0')]), /schema/)
  assert.throws(() => kernelToolDefinitions([...definitions, definitions[0]]), /schema/)
})
