import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { FoxTurnOutcome, modelFailure, probeModelRoute, checkSelectedModelRoute } from './pi-turn-outcome.mjs';
import { foxModelProxyTarget } from './dsh-model-proxy.mjs';

const failed = { role: 'assistant', content: [], stopReason: 'error', errorMessage: '403 Connection blocked by network allowlist key=secret' };
const success = { role: 'assistant', content: [{ type: 'text', text: 'ok' }], stopReason: 'stop' };

test('Pi terminated response streams keep their connection-interrupted category through settlement', () => {
  const assistant = { role: 'assistant', content: [{ type: 'text', text: '连接中断测试' }], stopReason: 'error', errorMessage: 'terminated' };
  const outcome = new FoxTurnOutcome();
  for (const event of [
    { type: 'message_end', message: assistant },
    { type: 'auto_retry_end', success: false, finalError: 'terminated' },
    { type: 'agent_end', messages: [assistant] },
  ]) {
    outcome.observe(event);
    assert.equal(outcome.failure.failureKind, 'connection_interrupted');
    assert.equal(outcome.failure.message, '模型服务连接中断，请检查网络或服务状态后重试。');
  }
  for (const errorMessage of ['TypeError: terminated', 'ECONNRESET', 'UND_ERR_SOCKET', 'socket hang up']) {
    assert.equal(modelFailure({ errorMessage }).failureKind, 'connection_interrupted');
  }
  assert.equal(modelFailure({ errorMessage: 'tool process terminated with exit code 1' }).failureKind, 'model_error');
  assert.equal(modelFailure({ errorMessage: 'ECONNREFUSED' }).failureKind, 'network_unreachable');
  outcome.observe({ type: 'message_end', message: success });
  assert.equal(outcome.failure, null);
});

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
  const disconnected = { ...failed, errorMessage: 'terminated' };
  assert.equal((await settle([{ type: 'message_end', message: disconnected }])).error.data.failureKind, 'connection_interrupted');
  assert.equal((await settle([{ type: 'message_end', message: disconnected }], true)).value, 'cancelled');
  assert.equal((await settle([{ type: 'message_end', message: failed }, { type: 'message_end', message: success }])).value, 'end_turn');
  const partial = await settle([{ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'partial' } }, { type: 'turn_end', message: failed }]);
  assert.equal(partial.error.data.failureKind, 'network_allowlist');
  assert.equal(partial.updates.filter(update => update.sessionUpdate === 'agent_message_chunk').length, 1);
  assert.equal((await settle([{ type: 'tool_execution_end', toolCallId: '', isError: false }])).value, 'end_turn');
});

const dshLoopPath = process.env.FOX_PATCHED_DSH_LOOP;
test('patched dsh cancels owned jobs and blocks notices until the next user prompt', { skip: !dshLoopPath && 'set FOX_PATCHED_DSH_LOOP to the staged patched bundle' }, () => {
  const source = readFileSync(dshLoopPath, 'utf8');
  const start = source.indexOf('var ReactLoopAgent = class {');
  const end = source.indexOf('\n};', start) + 3;
  assert.ok(start > 0 && end > start);
  const context = {};
  runInNewContext(source.slice(start, end) + '\nthis.Agent = ReactLoopAgent;', context);
  const agent = Object.create(context.Agent.prototype);
  const queued = [], killed = [];
  const notice = { source: { kind: 'tool-jobs' } };
  const jobs = {
    list: () => [
      { id: 'running', owner: 'session', status: 'running' },
      { id: 'stopping', owner: 'session', status: 'stopping' },
      { id: 'completed', owner: 'session', status: 'completed' },
      { id: 'foreign', owner: 'another', status: 'running' },
    ],
    kill(id, owner) { killed.push([id, owner]); agent.followup(notice); },
  };
  const abort = new AbortController();
  Object.assign(agent, {
    id: 'session', ctx: { get: () => jobs, logger: { warn: assert.fail } },
    phase: { kind: 'running', abort, wakeRequested: true },
    inbox: { clear() { queued.length = 0; }, splice(_target, _start, _remove, messages) { queued.push(...messages); } },
    wakeDriver() { this.wakes = (this.wakes ?? 0) + 1; },
  });
  agent.cancel({ kind: 'user' });
  assert.equal(abort.signal.aborted, true);
  assert.deepEqual(killed, [['running', 'session'], ['stopping', 'session']]);
  assert.equal(agent.wakes, undefined, 'job settlement during cancellation must not wake the loop');
  agent.phase = { kind: 'idle' };
  agent.followup(notice);
  agent.inject(notice);
  assert.equal(queued.length, 0);
  assert.equal(agent.wakes, undefined);
  const user = { source: { kind: 'user' } };
  agent.followup(user);
  assert.equal(agent.wakes, 1);
  assert.deepEqual(queued, [user]);
  agent.followup(notice);
  assert.equal(agent.wakes, 2, 'normal background completion remains available after explicit resume');
});

const dshJobsPath = process.env.FOX_PATCHED_DSH_JOBS;
test('patched dsh job delivery never wakes or injects explicitly killed work', { skip: !dshJobsPath && 'set FOX_PATCHED_DSH_JOBS to the staged patched bundle' }, () => {
  const source = readFileSync(dshJobsPath, 'utf8');
  const start = source.indexOf('ctx.jobs.events.subscribe({ owners: "scope" }');
  const end = source.indexOf('\n\t});', start) + 5;
  assert.ok(start > 0 && end > start);
  let settle;
  const messages = [];
  const owner = { status: 'idle', followup: message => messages.push(message), inject: message => messages.push(message) };
  const context = {
    ctx: { jobs: { events: { subscribe(_filter, listener) { settle = listener; } } }, get: () => ({ get: () => owner }) },
    killedByModel: new Set(), delivery: 'wakeup', wakeBudget: undefined, spentWakes: new WeakMap(),
    createUserMessage: value => value, fitCompletionNotice: () => 'job finished', completionSummary: () => 'job',
  };
  runInNewContext(source.slice(start, end), context);
  const event = { type: 'settled', job: { id: 'job', owner: 'session' }, awaited: false };
  settle({ ...event, cause: 'kill' });
  assert.equal(messages.length, 0, 'also suppressed after the next prompt has resumed');
  settle({ ...event, cause: 'producer' });
  assert.equal(messages.length, 1, 'ordinary completion still wakes the owner');
});
