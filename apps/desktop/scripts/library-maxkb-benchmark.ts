import { readFileSync, writeFileSync } from 'node:fs'
import { libraryFolders, libraryFolderCounts, normalizeLibraryPath, type LibraryFile } from '../src/features/local-knowledge/library-file-model'
const files: LibraryFile[] = JSON.parse(readFileSync('../../output/library-preview-fixes-20261016/maxkb-files.json', 'utf8'))
function before() { return libraryFolders(files).map(path => ({ path, count: files.filter(file => normalizeLibraryPath(file.path).startsWith(`${path}/`)).length })) }
function after() { return [...libraryFolderCounts(files)].sort(([a], [b]) => a.localeCompare(b, 'zh-CN')).map(([path, count]) => ({ path, count })) }
const bench = (fn: () => unknown) => { const samples = []; for (let i = 0; i < 12; i++) { const start = performance.now(); fn(); samples.push(performance.now() - start) } return samples.slice(2).sort((a, b) => a - b)[5] }
const a = before(), b = after()
if (JSON.stringify(a) !== JSON.stringify(b)) throw Error('counts differ')
const report = { files: files.length, folders: a.length, oldMedianMs: bench(before), newMedianMs: bench(after), oldRequests: Math.ceil(files.length / 100) + 1, newRequests: Math.floor(files.length / 2000) + 1 }
writeFileSync('../../output/library-preview-fixes-20261016/maxkb-benchmark.json', JSON.stringify(report, null, 2))
console.log(report)
