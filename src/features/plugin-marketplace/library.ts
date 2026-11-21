/**
 * Attention flag for an installed plugin (adapted from upstream library.ts).
 *
 * Only the read-only decision helper is ported: our marketplace view does not build a
 * merged "library" of catalog + installed sources, so the upstream path-normalisation
 * and source-deduplication code would be unused here.
 */
import type { InstalledPlugin, PluginUpdate as BasePluginUpdate } from './composable.ts'
import { canApplyPluginUpdate, pluginUpdateState, type PluginUpdate } from './updates.ts'

export function pluginNeedsAttention(plugin: InstalledPlugin, update?: BasePluginUpdate): boolean {
  if (plugin.removalPending) return true
  if (plugin.managed && (canApplyPluginUpdate(update as PluginUpdate) || pluginUpdateState(update as PluginUpdate) === 'error')) return true
  if (plugin.enabled === false) return false
  return plugin.prepared === false
    || plugin.loaded === false
    || plugin.availability === 'unavailable'
    || plugin.compatibility?.compatible === false
    || !!(plugin.error || plugin.dependencyError)
}
