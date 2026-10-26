import fs from 'node:fs/promises'
import path from 'node:path'
import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

if (process.platform !== 'win32') throw new Error('This installer test requires Windows and NSIS')
const root = fileURLToPath(new URL('..', import.meta.url))
const fixture = path.join(root, '.test-target', `sandbox-installer-${Date.now()}`)
const windows = path.join(fixture, 'windows')
const resources = path.join(fixture, 'resources/sandbox-installer')
await fs.mkdir(windows, { recursive: true })
await fs.mkdir(resources, { recursive: true })
await fs.copyFile(path.join(root, 'apps/desktop/src-tauri/windows/hooks.nsh'), path.join(windows, 'hooks.nsh'))
await fs.copyFile(path.join(root, 'apps/desktop/src-tauri/windows/install-sandbox.ps1'), path.join(resources, 'install-sandbox.ps1'))
await fs.writeFile(path.join(resources, 'runtime.zip'), 'fixture; no real runtime is executed')
await fs.writeFile(path.join(resources, 'package.nsh'), `!define FOX_SANDBOX_VERSION "0.1.3"\n!define FOX_SANDBOX_SHA256 "${'0'.repeat(64)}"\n`)
await fs.writeFile(path.join(fixture, 'test.nsi'), `
Unicode true
RequestExecutionLevel user
SilentInstall silent
!include LogicLib.nsh
!include "windows\\hooks.nsh"
Name "Fox sandbox failure fixture"
OutFile "test.exe"
InstallDir "$TEMP\\fox-installer-unused"
Section
  !insertmacro NSIS_HOOK_PREINSTALL
  FileOpen $1 "${fixture.replaceAll('$', '$$')}\\unexpected-success.txt" w
  FileWrite $1 "must not be reached"
  FileClose $1
SectionEnd
`)
const nsis = process.env.NSIS_DIR || path.join(process.env.LOCALAPPDATA, 'tauri/NSIS')
const cases = [
  { name: 'nonzero-exit', script: 'exit 7\n', error: /Sandbox setup exited with code 7/ },
  { name: 'false-success', script: `param($PackagePath, $ExpectedSha256, $Version, $ResultPath, $LogPath)
    [IO.File]::WriteAllText($ResultPath, '{"ok":false,"runtimeRoot":null,"srt":null}')
    exit 0\n`, error: /did not return a successful installation result/ },
]
for (const sample of cases) {
  await fs.writeFile(path.join(resources, 'enable-sandbox.ps1'), sample.script)
  execFileSync(path.join(nsis, 'makensis.exe'), ['/V2', 'test.nsi'], { cwd: fixture, windowsHide: true, stdio: 'pipe' })
  const data = path.join(fixture, sample.name)
  const result = spawnSync(path.join(fixture, 'test.exe'), ['/S'], {
    cwd: fixture, windowsHide: true, timeout: 60000,
    env: { ...process.env, ProgramData: data },
  })
  assert.ifError(result.error)
  assert.equal(result.status, 1, 'failed sandbox setup must fail the installer')
  await assert.rejects(fs.access(path.join(fixture, 'unexpected-success.txt')), /ENOENT/)
  const logs = path.join(data, 'Fox/logs')
  const names = await fs.readdir(logs)
  assert.equal(names.length, 1)
  assert.match(await fs.readFile(path.join(logs, names[0]), 'utf8'), sample.error)
  console.log(`PASS ${sample.name}: compiled NSIS hook returned 1, did not run the app step, and saved its log`)
}
console.log(`Fixture: ${fixture}`)
