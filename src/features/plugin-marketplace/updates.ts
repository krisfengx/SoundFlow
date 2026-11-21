/**
 * Plugin update decision layer (adapted from upstream pymss-studio).
 *
 * Upstream receives an explicit `status` / `target.schemaVersion` / `contentDigest`
 * from a newer worker. Our worker still reports `up_to_date`, `content_changed` and
 * the two version strings, so the same state machine is derived from those fields and
 * automatically prefers an explicit `status` once the backend starts sending one.
 */
import type { InstalledPlugin, PluginInventory, PluginUpdate as BasePluginUpdate } from './composable.ts'

export type PluginUpdateState =
  | 'unchecked' | 'update' | 'content' | 'source_changed'
  | 'older' | 'incomparable' | 'latest' | 'error'

export type PluginUpdate = BasePluginUpdate & {
  status?: PluginUpdateState
  target?: { schemaVersion?: number }
  contentDigest?: string
}

const UPSTREAM_STATES: readonly PluginUpdateState[] = [
  'unchecked', 'update', 'content', 'source_changed', 'older', 'incomparable', 'latest', 'error',
]

export function pluginUpdateState(update?: PluginUpdate): PluginUpdateState {
  if (!update) return 'unchecked'
  if (update.error) return 'error'
  if (update.status && UPSTREAM_STATES.includes(update.status)) return update.status
  if (update.up_to_date === true) return 'latest'
  if (update.up_to_date !== false) return 'unchecked'
  // Same version but the source files changed on the remote side.
  if (update.content_changed && update.local_version === update.remote_version) return 'content'
  return 'update'
}

export function canApplyPluginUpdate(update?: PluginUpdate): boolean {
  const state = pluginUpdateState(update)
  if (state !== 'update' && state !== 'content') return false
  // A future worker may advertise a target schema we cannot install yet.
  return update?.target?.schemaVersion === undefined || update.target.schemaVersion === 1
}

export function canCheckPluginUpdate(plugin: InstalledPlugin): boolean {
  return plugin.managed && plugin.prepared !== false && !plugin.removalPending
    && !!(plugin.url || plugin.preinstalled)
}

/** Fingerprint used to detect that the runtime or the installation changed under a batch. */
export function pluginUpdateKey(plugin: InstalledPlugin, runtime: PluginInventory['runtime']): string {
  return JSON.stringify([
    runtime.identity, runtime.pythonPath,
    plugin.name, plugin.managed, plugin.prepared, plugin.removalPending, plugin.version,
    plugin.source, plugin.url, plugin.resolvedCommit,
  ])
}

export type PluginUpdateSelection = {
  name: string
  version: string | null
  installationKey: string
  update: PluginUpdate
}

/** Deduplicated, currently updatable plugins for the batch dialog. */
export function selectPluginUpdates(
  inventory: PluginInventory | null,
  updates: Record<string, BasePluginUpdate>,
): PluginUpdateSelection[] {
  if (!inventory) return []
  const names = new Set<string>()
  return inventory.installed.flatMap(plugin => {
    if (names.has(plugin.name) || !canCheckPluginUpdate(plugin)) return []
    const update = updates[plugin.name] as PluginUpdate | undefined
    if (!canApplyPluginUpdate(update) || !update) return []
    names.add(plugin.name)
    return [{
      name: plugin.name,
      version: plugin.version,
      installationKey: pluginUpdateKey(plugin, inventory.runtime),
      update,
    }]
  })
}
