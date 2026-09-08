import net from 'node:net';
import { spawn } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';

const desktopRoot = fileURLToPath(new URL('../apps/desktop/', import.meta.url));
const normalize = (value) => {
  const normalized = value.replaceAll('\\', '/').replace(/\/$/, '');
  return process.platform === 'win32' ? normalized.toLowerCase() : normalized;
};

export function matchesProject(source, root) {
  // React Refresh identifies the actual served entry, unlike /@fs/ which may
  // expose files from other worktrees too. Fail closed if Vite changes format.
  const match = source.match(/getRefreshReg\(("(?:[^"\\]|\\.)*")\)/);
  if (!match) return false;
  try {
    return normalize(JSON.parse(match[1])) === normalize(path.join(root, 'src/main.tsx'));
  } catch {
    return false;
  }
}

function portOccupied(port) {
  return new Promise((resolve, reject) => {
    const socket = net.connect({ host: '127.0.0.1', port });
    socket.setTimeout(1500);
    socket.once('connect', () => { socket.destroy(); resolve(true); });
    socket.once('timeout', () => { socket.destroy(); reject(new Error('Port probe timed out')); });
    socket.once('error', (error) => {
      if (error.code === 'ECONNREFUSED') resolve(false);
      else reject(error);
    });
  });
}

export async function inspectDevServer({ port = 1421, root = desktopRoot } = {}) {
  if (!await portOccupied(port)) return 'available';
  try {
    const response = await fetch(`http://127.0.0.1:${port}/src/main.tsx`, {
      signal: AbortSignal.timeout(3000), redirect: 'error',
    });
    if (response.ok && matchesProject(await response.text(), root)) return 'reuse';
  } catch {
    // An occupied port is not permission to stop its owner or change devUrl.
  }
  throw new Error(`Port ${port} is occupied, but it is not the current Fox project's Vite server. Stop that server explicitly or use the matching project. No process was stopped.`);
}

export async function main() {
  const state = await inspectDevServer();
  if (state === 'reuse') {
    console.log('[Fox dev] Reusing this project\'s Vite server at http://127.0.0.1:1421');
    return;
  }
  // Keep pnpm's predev lifecycle (including generated profile avatars).
  const child = spawn('pnpm', ['dev'], {
    cwd: desktopRoot, stdio: 'inherit', shell: process.platform === 'win32',
  });
  child.once('error', (error) => { console.error(error.message); process.exitCode = 1; });
  child.once('exit', (code) => { process.exitCode = code ?? 1; });
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch((error) => { console.error(`[Fox dev] ${error.message}`); process.exitCode = 1; });
}
