export type InferenceParamMeta = {
  recommendedSampleStep?: number
  source?: string
  chunkSizeConstraint?: { step: number; offset: number; min: number }
  resolution?: ParameterResolutionMetadata
}

export type ParameterResolutionMetadata = {
  version: 1
  fingerprint: string
  defaults: Record<string, number | boolean>
  capabilities: Record<string, Record<string, unknown>>
  overlap: { kind: string; count?: number }
  [key: string]: unknown
}

const MAX_INFERENCE_SAMPLE_VALUE = 1_048_576
const INFERENCE_SAMPLE_STEP_MULTIPLIER = 10

/** Keep the v4 resolution identity without pulling in the full parameter editor. */
export function normalizeResolutionMetadata(value: unknown): ParameterResolutionMetadata | undefined {
  if (!value || typeof value !== 'object') return undefined
  const source = value as Record<string, unknown>
  if (source.version !== 1 || typeof source.fingerprint !== 'string' || !source.fingerprint) return undefined
  if (!source.defaults || typeof source.defaults !== 'object' || Array.isArray(source.defaults)) return undefined
  if (!source.capabilities || typeof source.capabilities !== 'object' || Array.isArray(source.capabilities)) return undefined
  const overlap = source.overlap && typeof source.overlap === 'object' && !Array.isArray(source.overlap)
    ? source.overlap as Record<string, unknown>
    : {}
  return {
    ...source,
    version: 1,
    fingerprint: source.fingerprint,
    defaults: source.defaults as Record<string, number | boolean>,
    capabilities: source.capabilities as Record<string, Record<string, unknown>>,
    overlap: {
      kind: typeof overlap.kind === 'string' ? overlap.kind : 'unknown',
      ...(typeof overlap.count === 'number' && Number.isFinite(overlap.count) && overlap.count > 0
        ? { count: overlap.count } : {}),
    },
  }
}

export function normalizeInferenceParamMeta(input: unknown): InferenceParamMeta | undefined {
  if (!input || typeof input !== 'object') return undefined
  const source = input as Record<string, unknown>
  const rawStep = Number(source.recommendedSampleStep)
  const resolution = normalizeResolutionMetadata(source.resolution)
  // A model may publish only a chunk-size lattice (or only v4 resolution metadata)
  // without a recommended sample step. Returning undefined here used to discard
  // those constraints entirely, so the UI fell back to unconstrained sliders.
  const hasStep = Number.isFinite(rawStep) && rawStep >= 1 && rawStep <= MAX_INFERENCE_SAMPLE_VALUE
  const rawSource = typeof source.source === 'string' ? source.source.trim() : ''
  const constraint = source.chunkSizeConstraint as Record<string, unknown> | undefined
  const step = Number(constraint?.step)
  const offset = Number(constraint?.offset)
  const min = Number(constraint?.min)
  const validConstraint = Number.isSafeInteger(step) && step > 0 && step <= MAX_INFERENCE_SAMPLE_VALUE
    && Number.isSafeInteger(offset) && offset >= 0 && offset < step
    && Number.isSafeInteger(min) && min > 0 && min <= MAX_INFERENCE_SAMPLE_VALUE
    && (min - offset) % step === 0
  if (!hasStep && !validConstraint && !resolution) return undefined
  return {
    ...(hasStep ? { recommendedSampleStep: Math.floor(rawStep) } : {}),
    ...(rawSource ? { source: rawSource } : {}),
    ...(validConstraint ? { chunkSizeConstraint: { step, offset, min } } : {}),
    ...(resolution ? { resolution } : {}),
  }
}

export function resolveInferenceSampleStep(meta: InferenceParamMeta | undefined, fallback: number) {
  const normalized = normalizeInferenceParamMeta(meta)
  if (!normalized?.recommendedSampleStep) return fallback
  const scaled = normalized.recommendedSampleStep * INFERENCE_SAMPLE_STEP_MULTIPLIER
  return scaled <= MAX_INFERENCE_SAMPLE_VALUE ? scaled : normalized.recommendedSampleStep
}

export function resolveInferenceChunkStep(meta: InferenceParamMeta | undefined, fallback: number) {
  return normalizeInferenceParamMeta(meta)?.chunkSizeConstraint?.step || resolveInferenceSampleStep(meta, fallback)
}

export function inferenceChunkSizeIssue(value: number | null | undefined, meta: InferenceParamMeta | undefined) {
  const constraint = normalizeInferenceParamMeta(meta)?.chunkSizeConstraint
  if (!constraint || value == null || value === 0) return undefined
  if (Number.isSafeInteger(value) && value >= constraint.min && (value - constraint.offset) % constraint.step === 0) {
    return undefined
  }
  const candidate = Number.isFinite(value) ? value : constraint.min
  const lower = Math.max(constraint.min, constraint.offset + Math.floor((candidate - constraint.offset) / constraint.step) * constraint.step)
  const upper = Math.max(constraint.min, constraint.offset + Math.ceil((candidate - constraint.offset) / constraint.step) * constraint.step)
  const values = [...new Set([lower, upper])].filter(value => value <= MAX_INFERENCE_SAMPLE_VALUE)
  if (!values.length) {
    values.push(constraint.offset + Math.floor((MAX_INFERENCE_SAMPLE_VALUE - constraint.offset) / constraint.step) * constraint.step)
  }
  return { values: values.join(' / ') }
}

export function alignInferenceStepChange(
  current: number | null,
  next: number | null,
  alignmentStep: number | undefined,
  controlStep: number,
  alignmentOffset = 0,
  alignmentMin = 0,
) {
  if (
    typeof current !== 'number'
    || !Number.isFinite(current)
    || typeof next !== 'number'
    || !Number.isFinite(next)
    || typeof alignmentStep !== 'number'
    || !Number.isFinite(alignmentStep)
    || alignmentStep < 1
    || (current >= alignmentMin && (current - alignmentOffset) % alignmentStep === 0)
  ) {
    return next
  }
  const delta = next - current
  if (Math.abs(delta) !== controlStep) return next
  return Math.max(alignmentMin, alignmentOffset + (delta > 0
    ? Math.ceil((current - alignmentOffset) / alignmentStep)
    : Math.floor((current - alignmentOffset) / alignmentStep)) * alignmentStep)
}
