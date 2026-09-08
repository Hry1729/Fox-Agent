import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { inspectDevServer, matchesProject } from './start-desktop-dev.mjs';

const root = '/fox/apps/desktop';
const entry = `window.$RefreshReg$ = RefreshRuntime.getRefreshReg("${root}/src/main.tsx");`;

test('only the exact served project entry is reusable', () => {
  assert.equal(matchesProject(entry, root), true);
  assert.equal(matchesProject(entry, '/other/apps/desktop'), false);
  assert.equal(matchesProject('<html>Another app</html>', root), false);
  assert.equal(matchesProject('getRefreshReg("broken\\x")', root), false);
});

async function withServer(handler, run) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  try { await run(port); } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

test('reuses the current project server', async () => {
  await withServer((req, res) => res.end(entry), async (port) => {
    assert.equal(await inspectDevServer({ port, root }), 'reuse');
  });
});

test('rejects a different project without stopping its server', async () => {
  await withServer((req, res) => res.end(entry), async (port) => {
    await assert.rejects(inspectDevServer({ port, root: '/other' }), /No process was stopped/);
    assert.equal((await fetch(`http://127.0.0.1:${port}`)).status, 200);
  });
});

test('rejects a failed entry response', async () => {
  await withServer((req, res) => { res.statusCode = 500; res.end(entry); }, async (port) => {
    await assert.rejects(inspectDevServer({ port, root }), /occupied/);
  });
});

test('an unused port permits normal startup', async () => {
  let releasedPort;
  await withServer((req, res) => res.end(), async (port) => { releasedPort = port; });
  assert.equal(await inspectDevServer({ port: releasedPort, root }), 'available');
});
