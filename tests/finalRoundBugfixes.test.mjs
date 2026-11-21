import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import vm from 'node:vm'
import ts from 'typescript'
import {
  normalizeInferenceParamMeta,
  normalizeResolutionMetadata,
  resolveInferenceChunkStep,
} from '../src/features/inference/sampleStep.ts'
import { segmentIntersectsRect, edgeScrollVelocity } from '../src/utils/selection.ts'

function sourceFunction(file, name, scope) {
  const source = readFileSync(new URL(file, import.meta.url), 'utf8')
  const parsed = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS)
  // Store helpers are nested inside defineStore(), not top-level declarations.
  let declaration
  const visit = node => {
    if (!declaration && ts.isFunctionDeclaration(node) && node.name?.text === name) declaration = node
    if (!declaration) ts.forEachChild(node, visit)
  }
  visit(parsed)
  assert.ok(declaration, `${name} must exist`)
  const code = ts.transpileModule(declaration.getText(parsed), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText
  return vm.runInNewContext(`${code}\n${name}`, scope)
}

test('a future inferenceParamsVersion is preserved, not silently downgraded to v3', () => {
  const normalize = sourceFunction('../src/stores/task.ts', 'normalizeRunConfig', {
    CURRENT_INFERENCE_PARAMS_VERSION: 3,
    normalizeOutputLayout: value => value,
    normalizeOutputNaming: value => value,
  })
  for (const version of [3, 4, 7]) {
    const result = normalize({ inferenceParamsVersion: version, inferenceParams: { chunk_size: 44100 } })
    assert.equal(result.inferenceParamsVersion, version, `version ${version} must survive`)
    // The function runs in a separate vm realm, so compare by value.
    assert.equal(JSON.stringify(result.inferenceParams), JSON.stringify({ chunk_size: 44100 }),
      'snapshot params must not be rewritten')
  }
  const legacy = normalize({ inferenceParams: { chunk_size: 44100 } })
  assert.equal(legacy.inferenceParamsVersion, 3)
  assert.equal(legacy.inferenceParams.standardize, true)
})

test('chunk lattice metadata survives when recommendedSampleStep is absent', () => {
  const onlyConstraint = { chunkSizeConstraint: { step: 32768, offset: 31744, min: 31744 } }
  const normalized = normalizeInferenceParamMeta(onlyConstraint)
  assert.deepEqual(normalized, onlyConstraint)
  assert.equal(resolveInferenceChunkStep(onlyConstraint, 1024), 32768)
  assert.equal(normalizeInferenceParamMeta({}), undefined)
  assert.equal(normalizeInferenceParamMeta({ recommendedSampleStep: 0 }), undefined)
})

test('v4 resolution metadata is retained and structurally validated', () => {
  const resolution = {
    version: 1, fingerprint: 'abc123', defaults: { chunk_size: 44100 },
    capabilities: { chunk_size: { kind: 'lattice', step: 1024 } },
    overlap: { kind: 'samples', count: 8 },
  }
  const meta = normalizeInferenceParamMeta({ resolution })
  assert.equal(meta?.resolution?.fingerprint, 'abc123')
  assert.equal(meta?.resolution?.overlap.kind, 'samples')
  assert.equal(normalizeResolutionMetadata({ version: 2, fingerprint: 'x' }), undefined)
  assert.equal(normalizeResolutionMetadata({ version: 1, fingerprint: '' }), undefined)
  assert.equal(normalizeResolutionMetadata({ version: 1, fingerprint: 'x', defaults: [], capabilities: {} }), undefined)
  assert.equal(normalizeInferenceParamMeta({ resolution: { version: 1, fingerprint: 'x' } }), undefined)
})

test('segmentIntersectsRect covers rows skipped between two fast pointer samples', () => {
  const rect = { left: 0, top: 100, right: 200, bottom: 120 }
  assert.equal(segmentIntersectsRect({ x: 50, y: 0 }, { x: 50, y: 300 }, rect), true,
    'a vertical sweep crossing the row must hit it even with no sample inside')
  assert.equal(segmentIntersectsRect({ x: 50, y: 300 }, { x: 50, y: 0 }, rect), true, 'reverse direction is symmetric')
  assert.equal(segmentIntersectsRect({ x: 50, y: 0 }, { x: 50, y: 50 }, rect), false, 'a segment ending above misses')
  assert.equal(segmentIntersectsRect({ x: 300, y: 0 }, { x: 300, y: 300 }, rect), false, 'outside the horizontal slab')
  assert.equal(segmentIntersectsRect({ x: 50, y: 110 }, { x: 60, y: 110 }, rect), true, 'a zero-delta axis inside still hits')
})

test('workflow queueMutation records, clears and classifies failures', async () => {
  class WorkflowRevisionConflictError extends Error { code = 'WORKFLOW_REVISION_CONFLICT' }
  class WorkflowEditorInUseError extends Error { code = 'WORKFLOW_EDITOR_IN_USE' }
  const isSaving = { value: false }
  const lastMutationError = { value: null }
  let pendingPersistCount = 0
  let mutationQueue = Promise.resolve()
  const queueMutation = sourceFunction('../src/stores/workflow.ts', 'queueMutation', {
    isSaving, lastMutationError,
    get pendingPersistCount() { return pendingPersistCount },
    set pendingPersistCount(value) { pendingPersistCount = value },
    get mutationQueue() { return mutationQueue },
    set mutationQueue(value) { mutationQueue = value },
    WorkflowRevisionConflictError, WorkflowEditorInUseError, Error,
  })

  await queueMutation(async () => 'saved')
  assert.equal(lastMutationError.value, null)
  assert.equal(isSaving.value, false)

  await assert.rejects(queueMutation(async () => { throw new Error('disk full') }), /disk full/)
  assert.equal(lastMutationError.value.code, 'WORKFLOW_MUTATION_FAILED')
  assert.equal(lastMutationError.value.message, 'disk full')
  assert.ok(lastMutationError.value.at)

  await assert.rejects(queueMutation(async () => { throw new WorkflowRevisionConflictError('changed') }), /changed/)
  assert.equal(lastMutationError.value.code, 'WORKFLOW_REVISION_CONFLICT')

  await queueMutation(async () => 'saved again')
  assert.equal(lastMutationError.value, null, 'a later success clears the stale error')
  assert.equal(pendingPersistCount, 0)
})

test('edgeScrollVelocity is bounded, directional and stops outside the container', () => {
  const rect = { left: 0, top: 0, right: 400, bottom: 300 }
  assert.equal(edgeScrollVelocity({ x: 200, y: 10 }, rect) < 0, true, 'top edge scrolls up')
  assert.equal(edgeScrollVelocity({ x: 200, y: 290 }, rect) > 0, true, 'bottom edge scrolls down')
  assert.equal(edgeScrollVelocity({ x: 200, y: 150 }, rect), 0, 'middle does not scroll')
  assert.equal(edgeScrollVelocity({ x: 500, y: 10 }, rect), 0, 'outside horizontally')
  assert.equal(edgeScrollVelocity({ x: 200, y: 400 }, rect), 0, 'outside vertically')
  for (const y of [-1000, 0, 1, 299, 1000]) {
    assert.ok(Math.abs(edgeScrollVelocity({ x: 200, y }, rect)) <= 600, 'velocity is clamped')
  }
})
