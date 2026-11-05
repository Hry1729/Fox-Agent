import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, existsSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { runInNewContext } from 'node:vm';
import { spawn } from 'node:child_process';
import { constants as osConstants } from 'node:os';
import { access as fsAccess } from 'node:fs/promises';
import { constants } from 'node:fs';
import { FoxConsoleDecoder, pipeFoxConsoleOutput } from './pi-console-output.mjs';

test('console decoding preserves UTF-8 and GBK characters across every byte boundary', () => {
  for (const [bytes, expected] of [
    [Buffer.from('中文 UTF-8 😀\r\n'), '中文 UTF-8 😀\r\n'],
    [Buffer.from('c0b4d7d4203132372e302e302e3120b5c4bbd8b8b40d0a', 'hex'), '来自 127.0.0.1 的回复\r\n'],
  ]) {
    for (let split = 0; split <= bytes.length; split++) {
      const decoder = new FoxConsoleDecoder('gbk');
      assert.equal(decoder.decode(bytes.subarray(0, split)) + decoder.decode(bytes.subarray(split)) + decoder.decode(undefined, true), expected);
    }
  }
});

const runtime = process.env.FOX_FOLLOWUP_RUNTIME;
const originalRuntime = process.env.FOX_ORIGINAL_RUNTIME;
const native = process.platform === 'win32' && runtime;
const importRuntime = (root, path) => import(pathToFileURL(resolve(root, path)).href);

test('patched Pi local shell streams Chinese ping and preserves UTF-8 structured output', { skip: !native }, async () => {
  const { createLocalShellOperations, createShellToolDefinition } = await importRuntime(runtime,
    'backends/pi/node_modules/@earendil-works/pi-coding-agent/dist/core/tools/bash.js');
  const ops = createLocalShellOperations('cmd', () => ({ shell: process.env.ComSpec, args: ['/d', '/c'] }));
  const chunks = [];
  const result = await ops.exec('ping -n 1 127.0.0.1', process.cwd(), { onData: (chunk) => chunks.push(chunk) });
  const text = Buffer.concat(chunks).toString('utf8');
  assert.equal(result.exitCode, 0);
  assert.match(text, /127\.0\.0\.1/);
  assert.ok(!text.includes('\ufffd'), text);
  if (/来自|回复/.test(text)) assert.match(text, /来自 127\.0\.0\.1 的回复/);
  const pwsh = createLocalShellOperations('pwsh', () => ({ shell: 'pwsh.exe', args: ['-NoProfile', '-NonInteractive', '-Command', '-'], commandTransport: 'stdin' }));
  const tool = createShellToolDefinition(process.cwd(), { name: 'bash', label: 'bash', shellName: 'pwsh', tempFilePrefix: 'fox-console-test' }, { operations: pwsh });
  const call = await tool.execute('utf8', { command: "[Console]::Write('中文 UTF-8 😀')" });
  assert.match(JSON.stringify(call), /中文 UTF-8 😀/);
  assert.ok(!JSON.stringify(call).includes('\ufffd'));
  const largeTool = createShellToolDefinition(process.cwd(), { name: 'bash', label: 'bash', shellName: 'pwsh', tempFilePrefix: 'fox-console-test' }, { operations: pwsh });
  const large = await largeTool.execute('large', { command: '$lines = 1..3000 | ForEach-Object { "中文内容-$_" }; $bytes = [Text.Encoding]::GetEncoding(936).GetBytes(($lines -join "`r`n")); [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)' });
  assert.ok(!large.isError, JSON.stringify(large));
  const full = readFileSync(large.details.fullOutputPath, 'utf8');
  assert.match(full, /中文内容-1\r?\n/);
  assert.match(full, /中文内容-3000/);
  assert.ok(!full.includes('\ufffd'));
});

test('bundled Pi shell preserves Chinese output when a real command is cancelled', { skip: !native }, async () => {
  const base = 'backends/pi/node_modules/@earendil-works/pi-coding-agent/dist/';
  const source = readFileSync(resolve(runtime, base, 'bundle/chunks/chunk-MM3LKT73.js'), 'utf8');
  const start = source.indexOf('function createLocalShellOperations(');
  const end = source.indexOf('function createLocalBashOperations(', start);
  assert.ok(start >= 0 && end > start);
  const shell = await importRuntime(runtime, base + 'utils/shell.js');
  const { waitForChildProcess } = await importRuntime(runtime, base + 'utils/child-process.js');
  const { createLocalShellOperations } = await importRuntime(runtime, base + 'core/tools/bash.js');
  const context = { spawn2: spawn, constants, fsAccess, process, osConstants, setTimeout, clearTimeout,
    pipeFoxConsoleOutput, waitForChildProcess, ...shell,
    // This test does not pass a timeout; use the public implementation's same default.
    resolveTimeoutMs: () => undefined };
  const factory = runInNewContext(source.slice(start, end) + ';createLocalShellOperations', context);
  for (const create of [factory, createLocalShellOperations]) {
    const controller = new AbortController();
    let output = '';
    const ops = create('cmd', () => ({ shell: process.env.ComSpec, args: ['/d', '/c'] }));
    await assert.rejects(ops.exec('ping -n 30 127.0.0.1', process.cwd(), {
      signal: controller.signal,
      onData: (bytes) => { output += bytes.toString('utf8'); if (output.includes('TTL=')) controller.abort(); },
    }), /aborted/);
    assert.match(output, /TTL=/);
    assert.ok(!output.includes('\ufffd'), output);
  }
});

test('dsh provisions a Modify-only owned workspace and keeps write restrictions', { skip: !native || !originalRuntime }, async () => {
  const testRoot = resolve(process.env.FOX_NATIVE_TEST_ROOT);
  mkdirSync(testRoot, { recursive: true });
  const root = mkdtempSync(join(testRoot, 'acl-'));
  const workspace = join(root, 'workspace');
  const outside = join(root, 'outside');
  mkdirSync(workspace); mkdirSync(outside);
  const setup = `
    $sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = [System.Security.AccessControl.DirectorySecurity]::new()
    $acl.SetOwner($sid)
    $acl.SetAccessRuleProtection($true, $false)
    $acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($sid, 'Modify', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
    Set-Acl -LiteralPath $env:FOX_ACL_TEST_WORKSPACE -AclObject $acl
    (Get-Acl -LiteralPath $env:FOX_ACL_TEST_WORKSPACE).Sddl
  `;
  const before = execFileSync('pwsh.exe', ['-NoProfile', '-NonInteractive', '-Command', setup], {
    env: { ...process.env, FOX_ACL_TEST_WORKSPACE: workspace }, encoding: 'utf8', windowsHide: true,
  });
  const path = 'backends/dsh/node_modules/@deepseek-ai/dsh-sandbox-windows-acl/lib/index.js';
  const original = await importRuntime(originalRuntime, path);
  const patched = await importRuntime(runtime, path);
  const sid = patched.workspaceWriteSid(workspace);
  const oldGrant = original.AclWriteGrant.create(sid);
  assert.throws(() => oldGrant.add(workspace, true), /Win32 5/);
  oldGrant.dispose();
  const sandbox = new patched.AclSandbox({ mode: 'workspace-write', writableDirs: [workspace], writeSid: sid, tempDir: null });
  try {
    await sandbox.init();
    const child = sandbox.spawn({ command: process.env.ComSpec, args: ['/d', '/c', 'echo inside>inside.txt'], cwd: workspace });
    assert.equal((await child.wait()).exitCode, 0);
    assert.match(readFileSync(join(workspace, 'inside.txt'), 'utf8'), /inside/);
    const denied = sandbox.spawn({ command: process.env.ComSpec, args: ['/d', '/c', `echo outside>"${join(outside, 'escape.txt')}"`], cwd: workspace });
    assert.notEqual((await denied.wait()).exitCode, 0);
    assert.equal(existsSync(join(outside, 'escape.txt')), false);
  } finally { sandbox.dispose(); }
  const readonly = new patched.AclSandbox({ mode: 'read-only', writableDirs: [], tempDir: null });
  try {
    await readonly.init();
    const denied = readonly.spawn({ command: process.env.ComSpec, args: ['/d', '/c', 'echo denied>readonly.txt'], cwd: workspace });
    assert.notEqual((await denied.wait()).exitCode, 0);
    assert.equal(existsSync(join(workspace, 'readonly.txt')), false);
  } finally { readonly.dispose(); }
  const after = execFileSync('pwsh.exe', ['-NoProfile', '-NonInteractive', '-Command', '(Get-Acl -LiteralPath $env:FOX_ACL_TEST_WORKSPACE).Sddl'], {
    env: { ...process.env, FOX_ACL_TEST_WORKSPACE: workspace }, encoding: 'utf8', windowsHide: true,
  });
  assert.match(after, /\(A;;WO;;;/);
  assert.equal(after.split('G:')[0], before.split('G:')[0]);
});
