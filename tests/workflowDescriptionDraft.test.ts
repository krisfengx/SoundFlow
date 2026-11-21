import assert from 'node:assert/strict'
import test from 'node:test'
import { createDescriptionDraft } from '../src/workflows/descriptionDraft.ts'

function makeDraft(write: (text: string) => Promise<string> = async text => text.trim(), read = () => 'original') {
  return createDescriptionDraft({ read, write, limit: 20, delay: 10000 })
}

test('flush persists the edited text and clears the dirty flag', async () => {
  let saved = ''
  const draft = makeDraft(async text => { saved = text.trim(); return saved })
  assert.equal(draft.dirty(), false)
  draft.update('  new description  ')
  assert.equal(draft.dirty(), true)
  assert.equal(await draft.flush(), true)
  assert.equal(saved, 'new description')
  assert.equal(draft.state.text, 'new description')
  assert.equal(draft.dirty(), false)
})

test('a failed save keeps the draft unconfirmed instead of losing the text', async () => {
  const draft = makeDraft(async () => { throw new Error('save failed') })
  draft.update('unsaved text')
  assert.equal(await draft.flush(), false)
  assert.equal(draft.state.unconfirmed, true)
  assert.equal(draft.state.text, 'unsaved text')
  assert.match(draft.state.error, /save failed/)
})

test('discard restores the source text', async () => {
  const draft = makeDraft(async () => { throw new Error('offline') })
  draft.update('unsaved text')
  await draft.flush()
  draft.discard()
  assert.equal(draft.state.text, 'original')
  assert.equal(draft.state.unconfirmed, false)
  assert.equal(draft.dirty(), false)
})

test('text over the limit is never sent to the writer', async () => {
  let calls = 0
  const draft = makeDraft(async text => { calls++; return text })
  draft.update('x'.repeat(21))
  assert.equal(draft.tooLong(), true)
  assert.equal(await draft.flush(), false)
  assert.equal(calls, 0)
})

test('synchronize adopts an external change when there is no local edit', async () => {
  let source = 'original'
  const draft = createDescriptionDraft({ read: () => source, write: async text => text, limit: 50, delay: 10000 })
  source = 'changed elsewhere'
  draft.synchronize()
  assert.equal(draft.state.text, 'changed elsewhere')
  assert.equal(draft.dirty(), false)
})
