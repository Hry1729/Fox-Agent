import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { FoxTurnOutcome, modelFailure, probeModelRoute, checkSelectedModelRoute } from './pi-turn-outcome.mjs';
import { foxModelProxyTarget } from './dsh-model-proxy.mjs';

const failed = { role: 'assistant', content: [], stopReason: 'error', errorMessage: '403 Connection blocked by network allowlist key=secret' };
const success = { role: 'assistant', content: [{ type: 'text', text: 'ok' }], stopReason: 'stop' };

test('model error survives partial output and tool completion', () => {
  const outcome = new FoxTurnOutcome();
  outcome.observe({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'partial' } });
  outcome.observe({ type: 'message_end', message: failed });
  outcome.observe({ type: 'tool_execution_end', isError: false });
  assert.equal(outcome.failure.failureKind, 'network_allowlist');
  assert.doesNotMatch(JSON.stringify(outcome.failure), /secret|key=/);
});
test('retry success clears earlier model failure', () => {
  const outcome = new FoxTurnOutcome();
  outcome.observe({ type: 'turn_end', message: failed });
  outcome.observe({ type: 'auto_retry_start' });
  assert.ok(outcome.failure);
  outcome.observe({ type: 'message_end', message: success });
  outcome.observe({ type: 'auto_retry_end', success: true });
  assert.equal(outcome.failure, null);
});
test('tool error is not a model error; retry exhaustion is', () => {
  const outcome = new FoxTurnOutcome();
  outcome.observe({ type: 'tool_execution_end', isError: true });
  assert.equal(outcome.failure, null);
  outcome.observe({ type: 'auto_retry_end', success: false, finalError: 'ECONNREFUSED token=private' });
  assert.equal(outcome.failure.failureKind, 'network_unreachable');
  assert.doesNotMatch(JSON.stringify(outcome.failure), /private/);
});
test('dsh override matches only exact scheme, host and port', () => {
  for (const origin of ['http://localhost:3425', 'http://127.0.0.1:3425', 'http://[::1]:3425']) {
    const env = { FOX_MODEL_PROXY_REQUIRED: '1', FOX_MODEL_ENDPOINT: origin };
    assert.equal(foxModelProxyTarget(new URL(origin + '/v1/messages'), env), true);
    assert.equal(foxModelProxyTarget(new URL(origin.replace(':3425', ':3426')), env), false);
    assert.equal(foxModelProxyTarget(new URL(origin.replace('http:', 'https:')), env), false);
    assert.equal(foxModelProxyTarget(new URL(origin), {}), false);
  }
});
test('sandbox transport preflight distinguishes transport from credentials and never follows redirects', async () => {
  const env = { FOX_MODEL_PROXY_REQUIRED: '1', FOX_MODEL_ENDPOINT: 'http://127.0.0.1:3425', HTTP_PROXY: 'http://127.0.0.1:8000' };
  let calls = 0;
  await probeModelRoute(env, async (url, options) => {
    calls++;
    assert.equal(url, env.FOX_MODEL_ENDPOINT);
    assert.equal(options.method, 'HEAD');
    assert.equal(options.maxRedirections, 0);
    assert.equal(options.headers, undefined);
    return { statusCode: 401 };
  });
  assert.equal(calls, 1);
  await assert.rejects(probeModelRoute({ ...env, HTTP_PROXY: '' }), /proxy is absent/);
  await assert.rejects(probeModelRoute(env, async () => { throw Error('ECONNREFUSED secret-key'); }), /network/);
});
test('transport preflight drains the response and observes body errors', async () => {
  const env = { FOX_MODEL_PROXY_REQUIRED: '1', FOX_MODEL_ENDPOINT: 'http://127.0.0.1:3425', HTTP_PROXY: 'http://127.0.0.1:8000' };
  let drained = false;
  await probeModelRoute(env, async () => ({ statusCode: 200, body: { destroy() { throw new Error('unsafe destroy'); }, async dump() { drained = true; } } }));
  assert.equal(drained, true);
  await assert.rejects(probeModelRoute(env, async () => ({ statusCode: 200, body: { async dump() { throw new Error('UND_ERR_ABORTED'); } } })), /network/);
});
test('the actual loaded pi model must retain the authorized origin', async () => {
  const env = { FOX_MODEL_PROXY_REQUIRED: '1', FOX_MODEL_ENDPOINT: 'http://localhost:3425' };
  await checkSelectedModelRoute({ async getState() { return { model: { baseUrl: 'http://localhost:3425/v1' } }; } }, env);
  await assert.rejects(checkSelectedModelRoute({ async getState() { return { model: { baseUrl: 'http://localhost:3426/v1' } }; } }, env), /endpoint_changed/);
  await assert.rejects(checkSelectedModelRoute({ async getState() { return {}; } }, env), /endpoint_changed/);
});

const patchedPath = process.env.FOX_PATCHED_PI_ACP;
test('pinned patched PiAcpSession emits legal prompt failures and preserves cancellation and retry', { skip: !patchedPath && 'set FOX_PATCHED_PI_ACP to the staged patched bundle' }, async () => {
  const source = readFileSync(patchedPath, 'utf8');
  const start = source.indexOf('var PiAcpSession = class {');
  const end = source.indexOf('\n};', start) + 3;
  assert.ok(start > 0 && end > start);
  const context = { FoxTurnOutcome, modelFailure, probeModelRoute: async () => {}, checkSelectedModelRoute: async () => {}, RequestError2: { internalError: (data, message) => Object.assign(new Error(message), { code: -32603, data }) } };
  runInNewContext(source.slice(start, end) + '\nthis.Session = PiAcpSession;', context);
  async function settle(events, cancel = false) {
    let dispatch;
    const updates = [];
    const session = new context.Session({ sessionId: 'fixture', cwd: '.', mcpServers: [], proc: { onEvent(fn) { dispatch = fn; }, async prompt() {} }, conn: { async sessionUpdate(value) { updates.push(value.update); } } });
    const result = new Promise((resolve, reject) => session.startTurn({ message: 'test', images: [], resolve, reject }));
    const observed = result.then(value => ({ value }), error => ({ error }));
    await Promise.resolve();
    for (const event of events) dispatch(event);
    session.cancelRequested = cancel;
    dispatch({ type: 'agent_settled' });
    dispatch({ type: 'agent_settled' }); // duplicate settlement cannot resolve twice
    const outcome = await observed;
    await session.flushEmits();
    return { ...outcome, updates };
  }
  const error = await settle([{ type: 'message_end', message: failed }]);
  assert.equal(error.error.code, -32603);
  assert.equal(error.error.data.failureKind, 'network_allowlist');
  assert.equal(error.value, undefined);
  assert.equal((await settle([{ type: 'message_end', message: failed }], true)).value, 'cancelled');
  assert.equal((await settle([{ type: 'message_end', message: failed }, { type: 'message_end', message: success }])).value, 'end_turn');
  const partial = await settle([{ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'partial' } }, { type: 'turn_end', message: failed }]);
  assert.equal(partial.error.data.failureKind, 'network_allowlist');
  assert.equal(partial.updates.filter(update => update.sessionUpdate === 'agent_message_chunk').length, 1);
  assert.equal((await settle([{ type: 'tool_execution_end', toolCallId: '', isError: false }])).value, 'end_turn');
});
