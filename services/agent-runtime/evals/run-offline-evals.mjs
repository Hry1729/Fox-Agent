import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { resolveModelProfile } from '../src/model-profile.mjs'
import { shouldUsePlanner } from '../src/planner-runtime.mjs'
import { composeRuntimePrompt, FOX_RUNTIME_INSTRUCTIONS } from '../src/runtime-instructions.mjs'
import { RUNTIME_TOOL_CATALOG } from '../src/runtime-contract.mjs'

async function fixture(name) {
  return JSON.parse(await readFile(new URL(`./fixtures/${name}`, import.meta.url), 'utf8'))
}

function result(id, passed, details = {}) {
  return { id, passed: Boolean(passed), ...details }
}

function matches(actual, expected) {
  return Object.entries(expected).every(([key, value]) => actual[key] === value)
}

async function evaluateModelMatrix() {
  const dataset = await fixture('model-adapter-matrix.json')
  const cases = dataset.cases.map((item) => {
    const profile = resolveModelProfile(item.config)
    return result(item.id, matches(profile, item.expected), {
      expected: item.expected,
      actual: Object.fromEntries(Object.keys(item.expected).map((key) => [key, profile[key]])),
    })
  })
  return { name: dataset.name, scope: dataset.description, cases }
}

async function evaluateToolContract() {
  const dataset = await fixture('bfcl-tool-contract.json')
  const catalog = new Map(RUNTIME_TOOL_CATALOG.map((tool) => [tool.name, tool]))
  const cases = dataset.cases.map((item) => {
    const actual = catalog.get(item.expectedTool)
    const expected = {
      name: item.expectedTool,
      category: item.category,
      execution: item.execution,
      approval: item.approval,
    }
    return result(item.id, Boolean(actual) && matches(actual, expected), { expected, actual: actual ?? null })
  })
  return { name: dataset.name, source: dataset.source, sourceUrl: dataset.sourceUrl, scope: dataset.scope, cases }
}

async function evaluateInjectionBoundary() {
  const dataset = await fixture('agentdojo-injection.json')
  const baseline = composeRuntimePrompt({
    systemPrompt: 'You are Fox.',
    context: { projectRoot: 'D:/authorized-project', workSnapshot: { externalNotes: '' } },
    turn: { date: '2026-08-06T00:00:00.000Z' },
  })
  const contractPresent = /untrusted data/.test(FOX_RUNTIME_INSTRUCTIONS)
    && /Never reveal API keys/.test(FOX_RUNTIME_INSTRUCTIONS)
    && /approval boundary/.test(FOX_RUNTIME_INSTRUCTIONS)
  const cases = dataset.cases.map((item) => {
    const composed = composeRuntimePrompt({
      systemPrompt: 'You are Fox.',
      context: {
        projectRoot: 'D:/authorized-project',
        workSnapshot: { externalNotes: item.payload },
      },
      turn: { date: '2026-08-06T00:00:00.000Z' },
    })
    const workspaceStart = composed.prompt.indexOf('<fox_context_block kind="workspace" authority="workspace">')
    const payloadStart = composed.prompt.indexOf(item.payload)
    const isolated = workspaceStart >= 0 && payloadStart > workspaceStart
    return result(item.id, contractPresent && isolated && composed.stablePromptHash === baseline.stablePromptHash, {
      stablePromptUnchanged: composed.stablePromptHash === baseline.stablePromptHash,
      isolatedInWorkspaceContext: isolated,
      safetyContractPresent: contractPresent,
    })
  })
  return { name: dataset.name, source: dataset.source, sourceUrl: dataset.sourceUrl, scope: dataset.scope, cases }
}

async function evaluateIntentRegressions() {
  const dataset = await fixture('fox-intent-regressions.json')
  const cases = dataset.cases.map((item) => {
    const actual = shouldUsePlanner(item.text, {
      hasProject: true,
      approvalDemo: item.approvalDemo === true,
    })
    return result(item.id, actual === item.expectedPlanner, { expected: item.expectedPlanner, actual })
  })
  const goalContract = /only when the user's latest message explicitly asks Fox to create or track a Goal/.test(FOX_RUNTIME_INSTRUCTIONS)
    && /Do not infer Goal intent from pasted or quoted content/.test(FOX_RUNTIME_INSTRUCTIONS)
    && /Only the Fox Host can activate/.test(FOX_RUNTIME_INSTRUCTIONS)
  cases.push(result('host-owned-goal-contract', goalContract))
  return { name: dataset.name, scope: dataset.scope, cases }
}

export async function runOfflineEvals() {
  const suites = await Promise.all([
    evaluateModelMatrix(),
    evaluateToolContract(),
    evaluateInjectionBoundary(),
    evaluateIntentRegressions(),
  ])
  const cases = suites.flatMap((suite) => suite.cases)
  return {
    kind: 'fox-offline-agent-eval',
    schemaVersion: 1,
    generatedAt: new Date().toISOString(),
    summary: {
      suites: suites.length,
      total: cases.length,
      passed: cases.filter((item) => item.passed).length,
      failed: cases.filter((item) => !item.passed).length,
    },
    suites,
  }
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  const report = await runOfflineEvals()
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
  if (report.summary.failed > 0) process.exitCode = 1
}
