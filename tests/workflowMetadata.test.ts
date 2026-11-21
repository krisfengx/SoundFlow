import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createWorkflowMetadata,
  normalizeWorkflowMetadata,
  parseWorkflowFile,
  resolveWorkflowDescription,
  serializeWorkflowFile,
} from '../src/workflows/metadata.ts'

const definition = { name: 'demo', nodes: [{ id: 1 }] }

test('a serialized workflow round-trips its definition and notes', () => {
  const metadata = createWorkflowMetadata(undefined, true)
  metadata.notes = 'What this workflow does'
  const text = serializeWorkflowFile(definition, metadata)
  const parsed = parseWorkflowFile(text)
  assert.deepEqual(parsed.definition, definition)
  assert.equal(parsed.metadata?.guid, metadata.guid)
  assert.equal(resolveWorkflowDescription({ metadata: parsed.metadata }), 'What this workflow does')
  assert.equal(text.startsWith('{\n  "_metadata"'), true)
})

test('legacy exports without _metadata still import unchanged', () => {
  const parsed = parseWorkflowFile(JSON.stringify(definition))
  assert.deepEqual(parsed.definition, definition)
  assert.equal(parsed.metadata, undefined)
  assert.equal(resolveWorkflowDescription({ metadata: parsed.metadata }), '')
})

test('an invalid _metadata block is rejected instead of being dropped silently', () => {
  assert.throws(() => parseWorkflowFile(JSON.stringify({ _metadata: { guid: 'nope' }, ...definition })), /Invalid workflow metadata/)
  assert.equal(normalizeWorkflowMetadata({ guid: 'nope' }), undefined)
  assert.equal(normalizeWorkflowMetadata({ guid: '00000000-0000-4000-8000-000000000000', author: 'x'.repeat(201) }), undefined)
})

test('description falls back to the plain description field when there are no notes', () => {
  assert.equal(resolveWorkflowDescription({ description: '  legacy text  ' }), 'legacy text')
  assert.equal(resolveWorkflowDescription({ description: 'legacy', metadata: { guid: '00000000-0000-4000-8000-000000000000', notes: 'notes' } }), 'notes')
})
