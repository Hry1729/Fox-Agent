// Copied beside the pinned pi-acp bundle by runtime_patches.py.
// Error payloads deliberately contain categories only, never vendor bodies,
// request URLs, headers, prompts, or credential values.
export function modelFailure(error) {
  const text = String(error?.errorMessage ?? error?.message ?? error ?? '');
  const failureKind = /endpoint.changed/i.test(text) ? 'endpoint_changed'
    : /allowlist|blocked.by.network|proxy.*403/i.test(text) ? 'network_allowlist'
    : /401|unauthori[sz]ed|authentication|api.?key/i.test(text) ? 'authentication'
    : /429|rate.?limit/i.test(text) ? 'rate_limit'
    : /connection|fetch failed|ECONN|ENOTFOUND|ETIMEDOUT|proxy|network|abort.*connect/i.test(text) ? 'network_unreachable'
    : 'model_error';
  const messages = {
    endpoint_changed: 'pi 当前会话的模型端点已变化；请重新应用已授权的模型配置后重试',
    network_allowlist: '模型请求被沙箱网络允许列表阻止',
    authentication: '模型服务拒绝了登录凭据',
    rate_limit: '模型服务达到请求限额',
    network_unreachable: '沙箱内无法通过受管代理连接模型服务',
    model_error: '模型服务未能完成本轮请求',
  };
  return { code: 'agent_backend_model_failed', failureKind, redacted: true, message: messages[failureKind] };
}

export class FoxTurnOutcome {
  failure = null;
  observe(event) {
    const assistant = event.message?.role === 'assistant' ? event.message : null;
    const delta = event.assistantMessageEvent;
    if (event.type === 'error') this.failure = modelFailure(event.error ?? event);
    if (event.type === 'message_update' && delta?.type === 'error') this.failure = modelFailure(delta.error ?? assistant ?? delta);
    if ((event.type === 'message_end' || event.type === 'turn_end') && assistant) {
      if (assistant.stopReason === 'error' || assistant.stopReason === 'aborted') this.failure = modelFailure(assistant);
      else if (assistant.stopReason) this.failure = null;
    }
    if (event.type === 'auto_retry_end' && event.success === false) this.failure = modelFailure(event.finalError);
    if (event.type === 'agent_end') {
      const last = event.messages?.filter(message => message.role === 'assistant').at(-1);
      if (last?.stopReason === 'error' || last?.stopReason === 'aborted') this.failure = modelFailure(last);
      else if (last?.stopReason) this.failure = null;
    }
    // tool_execution_end.isError is a tool result the model can handle.
    // A subsequent successful assistant message is what clears a retry error.
  }
}

export async function checkSelectedModelRoute(proc, env = process.env) {
  if (env.FOX_MODEL_PROXY_REQUIRED !== '1' || !env.FOX_MODEL_ENDPOINT) return;
  const state = await proc.getState();
  try {
    const actual = new URL(state?.model?.baseUrl);
    const expected = new URL(env.FOX_MODEL_ENDPOINT);
    if (actual.origin === expected.origin) return;
  } catch {}
  throw new Error('endpoint_changed');
}

/** One transport probe per pi process, with no credentials or model call.
 * HTTP 401/404 still proves transport reachability; it does not verify a model.
 */
export async function probeModelRoute(env = process.env, transport) {
  if (env.FOX_MODEL_PROXY_REQUIRED !== '1' || !env.FOX_MODEL_ENDPOINT) return;
  const proxy = env.https_proxy || env.HTTPS_PROXY || env.http_proxy || env.HTTP_PROXY;
  if (!proxy) throw new Error('network: managed sandbox proxy is absent');
  let dispatcher;
  try {
    if (!transport) {
      const undici = await import('undici');
      dispatcher = new undici.EnvHttpProxyAgent({ noProxy: '' });
      transport = (url, options) => undici.request(url, { ...options, dispatcher });
    }
    const response = await transport(env.FOX_MODEL_ENDPOINT, {
      method: 'HEAD', maxRedirections: 0, headersTimeout: 5000, bodyTimeout: 5000,
      signal: AbortSignal.timeout(6000),
    });
    // undici's BodyReadable emits an error when destroy() aborts a request.
    // Await its drain so cancellation/errors stay in this promise chain.
    await response.body?.dump?.();
    if (response.statusCode === 407) throw new Error('network: sandbox proxy rejected the route');
  } catch (error) {
    // The caller converts this to a category, never forwards raw error data.
    throw new Error(`network: ${modelFailure(error).failureKind}`);
  } finally { await dispatcher?.close(); }
}
