import test from 'node:test'
import assert from 'node:assert/strict'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { collectPaths, formatReport } from '../scripts/paths-check.mjs'

const here = fileURLToPath(import.meta.url)
const root = path.dirname(path.dirname(here))
const missingPython = path.join(path.dirname(here), '__missing-bootstrap-python.exe')

test('a deleted absolute interpreter path is reported instead of failing later as os error 3', () => {
  const report = formatReport({ SOUNDFLOW_PYTHON: missingPython }, root)
  assert.ok(report.strays.some(row => row.name === 'tool:SOUNDFLOW_PYTHON'))
  assert.ok(report.lines.some(line => line.includes('MISS') && line.includes(missingPython)))
  assert.equal(collectPaths({ SOUNDFLOW_PYTHON: missingPython }, root)
    .find(row => row.name === 'tool:SOUNDFLOW_PYTHON').source, 'tool')
})

test('an existing interpreter path is accepted even when it is outside the checkout', () => {
  const report = formatReport({ SOUNDFLOW_PYTHON: here }, root)
  assert.ok(!report.strays.some(row => row.name === 'tool:SOUNDFLOW_PYTHON'))
})

test('a bare command name is resolved through PATH and is not treated as a missing file', () => {
  const report = formatReport({ SOUNDFLOW_PYTHON: 'python' }, root)
  assert.ok(!report.strays.some(row => row.name === 'tool:SOUNDFLOW_PYTHON'))
})
