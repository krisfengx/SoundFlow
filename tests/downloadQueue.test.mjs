import test from 'node:test'
import assert from 'node:assert/strict'
import {
  BATCH_DOWNLOAD_CONCURRENCY,
  enqueueDownloads,
  isActiveDownloadStatus,
  takeQueuedDownloads,
} from '../src/utils/downloadQueue.ts'

test('enqueue skips duplicates, empty names and models that cannot be queued', () => {
  const queued = enqueueDownloads(['a'], ['a', 'b', '', 'c', 'b', 'd'], name => name === 'c')
  assert.deepEqual(queued, ['a', 'b', 'd'])
  assert.equal(BATCH_DOWNLOAD_CONCURRENCY, 2)
})

test('a manual download occupies a slot, so queued downloads wait', () => {
  assert.deepEqual(takeQueuedDownloads(['a', 'b', 'c'], 0).start, ['a', 'b'])
  assert.deepEqual(takeQueuedDownloads(['a', 'b', 'c'], 1).start, ['a'])
  assert.deepEqual(takeQueuedDownloads(['a', 'b', 'c'], 2).start, [])
})

test('finishing a download frees a slot and returns the unchanged remainder', () => {
  const queue = ['a', 'b', 'c', 'd']
  const first = takeQueuedDownloads(queue, 1)
  assert.deepEqual(first, { start: ['a'], rest: ['b', 'c', 'd'] })
  const afterFinish = takeQueuedDownloads(first.rest, 1)
  assert.deepEqual(afterFinish, { start: ['b'], rest: ['c', 'd'] })
})

test('only preparing and downloading count as active', () => {
  for (const status of ['preparing', 'downloading']) assert.equal(isActiveDownloadStatus(status), true)
  for (const status of ['done', 'error', 'cancelled', 'paused', 'interrupted', undefined, '']) {
    assert.equal(isActiveDownloadStatus(status), false)
  }
})
