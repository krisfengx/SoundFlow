#!/usr/bin/env node
/**
 * Executable form of the "everything stays inside the checkout" contract.
 *
 * dev-env.mjs imports formatReport() to check the environment it is about to
 * hand to a child process. Running this file directly checks the *current*
 * process environment instead, which is useful from CI or a shell that was
 * already launched through `dev-env.mjs run -- ...`.
 *
 * Path keys that merely point at borrowed system toolchains (RUSTUP_HOME) are
 * intentionally not checked: they are read-only tool selection, not project writes.
 * SOUNDFLOW_PYTHON is also allowed to point outside the checkout, but an explicit
 * interpreter *path* that no longer exists silently breaks every bootstrap worker
 * command (seen as `os error 3`), so its existence is checked separately below.
 */
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const DEFAULT_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

const ENV_KEYS = [
  'PIP_CACHE_DIR', 'CARGO_HOME', 'npm_config_cache', 'PNPM_HOME',
  'HF_HOME', 'TORCH_HOME', 'XDG_CACHE_HOME', 'MODELSCOPE_CACHE',
  'TMP', 'TEMP', 'TMPDIR',
  'SOUNDFLOW_RUNTIME_ENVS_OVERRIDE', 'PYMSS_PLUGINS_DIR',
  'SOUNDFLOW_DATA_ROOT', 'PYMSS_MODEL_DIR',
  'PYMSS_USER_MODELS', 'SOUNDFLOW_DEBUG_DIR',
  'SOUNDFLOW_RUNTIME_ENVS_DIR', 'SOUNDFLOW_ACTIVE_RUNTIME_FILE',
  'SOUNDFLOW_BUNDLED_RUNTIME_ENVS_DIR',
]

/** Mirror of python/_project_paths.py::default_paths() for display purposes. */
function projectDefaults(root) {
  return {
    'default:data_root': path.join(root, '.soundflow'),
    'default:models_dir': path.join(root, '.soundflow', 'models'),
    'default:hf_home': path.join(root, '.soundflow', 'models', '_cache', 'hf'),
    'default:torch_home': path.join(root, '.soundflow', 'models', '_cache', 'torch'),
    'default:modelscope_cache': path.join(root, '.soundflow', 'models', '_cache', 'modelscope'),
    'default:settings_dir': path.join(root, '.soundflow', 'settings'),
    'default:debug_dir': path.join(root, '.soundflow', 'debug'),
    'default:plugins_dir': path.join(root, '.soundflow', 'plugins'),
    'default:user_models_file': path.join(root, '.soundflow', 'settings', 'user_models.json'),
    'default:runtime_envs_dir': path.join(root, '.venv', 'python', 'runtime-envs'),
    'default:scratch_dir': path.join(root, '.soundflow', 'temp'),
  }
}

/** Read-only tool variables whose *existence* matters even though their location does not. */
const TOOL_PATH_KEYS = ['SOUNDFLOW_PYTHON']

function missingToolPaths(env) {
  const rows = []
  for (const key of TOOL_PATH_KEYS) {
    const value = String(env[key] || '').trim()
    if (!value) continue
    // A bare command such as `python` is resolved through PATH and is not a file path.
    const looksLikePath = value.includes('/') || value.includes('\\') || /^[A-Za-z]:/.test(value)
    if (!looksLikePath || fs.existsSync(value)) continue
    rows.push({ name: `tool:${key}`, value, source: 'tool', inside: false })
  }
  return rows
}

export function isInside(root, value) {
  if (!value) return false
  const rel = path.relative(path.resolve(root), path.resolve(String(value)))
  return rel === '' || (rel !== '..' && !rel.startsWith('..' + path.sep) && !path.isAbsolute(rel))
}

export function collectPaths(env = process.env, root = DEFAULT_ROOT) {
  const rows = []
  for (const key of new Set(ENV_KEYS)) {
    if (env[key]) rows.push({ name: key, value: String(env[key]), source: 'env', inside: isInside(root, env[key]) })
  }
  for (const [name, value] of Object.entries(projectDefaults(root))) {
    rows.push({ name, value, source: 'default', inside: isInside(root, value) })
  }
  rows.push(...missingToolPaths(env))
  return rows
}

export function formatReport(env = process.env, root = DEFAULT_ROOT) {
  const rows = collectPaths(env, root)
  const lines = rows.map(row =>
    `  ${row.source === 'tool' ? 'MISS' : row.inside ? 'OK ' : 'OUT'}  ${row.name.padEnd(38)} ${row.value}`
  )
  return { rows, lines, strays: rows.filter(row => !row.inside) }
}

const invokedDirectly = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
if (invokedDirectly) {
  const { lines, strays } = formatReport()
  console.log(`\n  SoundFlow path contract  |  ${DEFAULT_ROOT}`)
  console.log('  (checks the current process env; use `pnpm env:paths` for the pinned launcher env)\n')
  for (const line of lines) console.log(line)
  console.log(`\n  RESULT: ${strays.length ? `FAIL - ${strays.length} path(s) outside the checkout or missing tool` : 'OK - all paths inside the checkout'}\n`)
  process.exit(strays.length ? 1 : 0)
}
