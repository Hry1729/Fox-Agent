// Only the exact endpoint supplied by the Host may override dsh's loopback
// bypass. All other local control endpoints keep the upstream direct route.
export function foxModelProxyTarget(url, env = process.env) {
  if (env.FOX_MODEL_PROXY_REQUIRED !== '1' || !env.FOX_MODEL_ENDPOINT) return false;
  try {
    const target = new URL(env.FOX_MODEL_ENDPOINT);
    const port = value => value.port || (value.protocol === 'https:' ? '443' : '80');
    return url.protocol === target.protocol && url.hostname === target.hostname && port(url) === port(target);
  } catch { return false; }
}
