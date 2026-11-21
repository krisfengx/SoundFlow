import assert from 'node:assert/strict'
import test from 'node:test'
import {
  canApplyPluginUpdate,
  canCheckPluginUpdate,
  pluginUpdateKey,
  pluginUpdateState,
  selectPluginUpdates,
} from '../src/features/plugin-marketplace/updates.ts'
import { pluginNeedsAttention } from '../src/features/plugin-marketplace/library.ts'
import type { InstalledPlugin, PluginInventory } from '../src/features/plugin-marketplace/composable.ts'

const runtime = { identity: 'cpu', pythonPath: 'E:/python.exe' }

function plugin(patch: Partial<InstalledPlugin> = {}): InstalledPlugin {
  return {
    name: 'demo', version: '1.0.0', source: 'url', url: 'https://example.com/plugin',
    path: 'E:/plugins/demo', loaded: true, managed: true, error: null,
    operations: [], prepared: true, ...patch,
  }
}

function inventory(installed: InstalledPlugin[]): PluginInventory {
  return { installed, pluginsDir: 'E:/plugins', runtime }
}

test('the seven states are derived from the fields our worker actually returns', () => {
  assert.equal(pluginUpdateState(undefined), 'unchecked')
  assert.equal(pluginUpdateState({ error: 'offline' }), 'error')
  assert.equal(pluginUpdateState({ up_to_date: true }), 'latest')
  assert.equal(pluginUpdateState({ up_to_date: false, remote_version: '2.0.0' }), 'update')
  assert.equal(pluginUpdateState({
    up_to_date: false, content_changed: true, local_version: '1.0.0', remote_version: '1.0.0',
  }), 'content')
  assert.equal(pluginUpdateState({}), 'unchecked')
  // A future worker sends an explicit status; it wins over the derived one.
  assert.equal(pluginUpdateState({ status: 'older', up_to_date: false }), 'older')
  assert.equal(pluginUpdateState({ status: 'source_changed' }), 'source_changed')
  assert.equal(pluginUpdateState({ status: 'incomparable' }), 'incomparable')
})

test('only update/content states can be applied, and only with a supported target schema', () => {
  assert.equal(canApplyPluginUpdate({ up_to_date: false }), true)
  assert.equal(canApplyPluginUpdate({ up_to_date: false, content_changed: true, local_version: '1', remote_version: '1' }), true)
  assert.equal(canApplyPluginUpdate({ up_to_date: true }), false)
  assert.equal(canApplyPluginUpdate({ error: 'offline' }), false)
  assert.equal(canApplyPluginUpdate({ up_to_date: false, target: { schemaVersion: 2 } }), false)
  assert.equal(canApplyPluginUpdate({ up_to_date: false, target: { schemaVersion: 1 } }), true)
})

test('checking requires a managed, prepared installation with a resolvable source', () => {
  assert.equal(canCheckPluginUpdate(plugin()), true)
  assert.equal(canCheckPluginUpdate(plugin({ url: null, preinstalled: true })), true)
  assert.equal(canCheckPluginUpdate(plugin({ url: null })), false)
  assert.equal(canCheckPluginUpdate(plugin({ managed: false })), false)
  assert.equal(canCheckPluginUpdate(plugin({ prepared: false })), false)
  assert.equal(canCheckPluginUpdate(plugin({ removalPending: true })), false)
})

test('the installation fingerprint changes with the runtime and the installed version', () => {
  const base = pluginUpdateKey(plugin(), runtime)
  assert.notEqual(pluginUpdateKey(plugin(), { ...runtime, identity: 'cuda' }), base)
  assert.notEqual(pluginUpdateKey(plugin({ version: '1.0.1' }), runtime), base)
  assert.notEqual(pluginUpdateKey(plugin({ resolvedCommit: 'abc' }), runtime), base)
  assert.equal(pluginUpdateKey(plugin(), runtime), base)
})

test('batch selection deduplicates names and skips plugins without an applicable update', () => {
  const installed = [
    plugin({ name: 'a' }), plugin({ name: 'a' }),
    plugin({ name: 'b' }), plugin({ name: 'c', prepared: false }),
  ]
  const selected = selectPluginUpdates(inventory(installed), {
    a: { up_to_date: false, remote_version: '2.0.0' },
    b: { up_to_date: true },
    c: { up_to_date: false },
  })
  assert.deepEqual(selected.map(item => item.name), ['a'])
  assert.equal(selected[0].installationKey, pluginUpdateKey(installed[0], runtime))
  assert.deepEqual(selectPluginUpdates(null, {}), [])
})

test('attention covers broken installs but not a plain available update', () => {
  assert.equal(pluginNeedsAttention(plugin({ prepared: false }), undefined), true)
  assert.equal(pluginNeedsAttention(plugin({ dependencyError: 'missing torch' }), undefined), true)
  assert.equal(pluginNeedsAttention(plugin({ removalPending: true }), undefined), true)
  assert.equal(pluginNeedsAttention(plugin(), { up_to_date: true }), false)
  assert.equal(pluginNeedsAttention(plugin(), { up_to_date: false }), true)
  assert.equal(pluginNeedsAttention(plugin({ enabled: false, prepared: false }), undefined), false)
})
