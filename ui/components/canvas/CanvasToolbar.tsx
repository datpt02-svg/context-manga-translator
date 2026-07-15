'use client'

import {
  LanguagesIcon,
  LoaderCircleIcon,
  ScanIcon,
  ScanTextIcon,
  TypeIcon,
  Wand2Icon,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import { getConfig, getCurrentLlm, putCurrentLlm, startPipeline, useGetCurrentLlm } from '@/lib/api/default/default'
import { pipelineOptions } from '@/lib/pipeline'
import { useJobsStore } from '@/lib/stores/jobsStore'
import { useSelectionStore } from '@/lib/stores/selectionStore'

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function CanvasToolbar() {
  return (
    <div className='flex items-center gap-2 border-b border-border/60 bg-card px-3 py-2 text-xs text-foreground'>
      <WorkflowButtons />
      <div className='flex-1' />
    </div>
  )
}

/** Currently-busy step (derived from jobsStore). */
function useCurrentStep(): string | null {
  const jobs = useJobsStore((s) => s.jobs)
  for (const j of Object.values(jobs)) {
    if (j.status === 'running' && j.progress?.step) return String(j.progress.step)
  }
  return null
}

function useIsProcessing(): boolean {
  const jobs = useJobsStore((s) => s.jobs)
  return Object.values(jobs).some((j) => j.status === 'running')
}

function WorkflowButtons() {
  const { t } = useTranslation()
  const { data: llmState } = useGetCurrentLlm()
  const llmReady = llmState?.status === 'ready'
  const pageId = useSelectionStore((s) => s.pageId)
  const hasPage = pageId !== null
  const isProcessing = useIsProcessing()
  const currentStep = useCurrentStep()

  /**
   * Run a pipeline step (or a small chain). `GET /config` is the single
   * source of truth for engine ids — every field has a serde default in
   * the Rust `PipelineConfig`, so we trust what the server returns and
   * never hard-code fallbacks here.
   *
   * Detect is the only multi-engine button; it bundles detector +
   * segmenter + font-detector so the subsequent single-engine steps
   * (OCR / Inpaint / Render) find their inputs already on the page. The
   * backend driver skips any step whose artifact is already satisfied,
   * so re-running is idempotent.
   */
  const runStep = async (
    pick: (p: NonNullable<Awaited<ReturnType<typeof getConfig>>['pipeline']>) => string[],
  ) => {
    if (!pageId) { console.warn('no page'); return }
    const cfg = await getConfig()
    if (!cfg.pipeline) { console.warn('no pipeline config'); return }
    const steps = pick(cfg.pipeline).filter((s): s is string => !!s)
    console.log('runStep steps:', steps)
    if (steps.length === 0) return
    // auto-load LLM/OCR provider based on the step we're about to run
    const needsLlm = steps.some((s) => s.endsWith('llm') || s.startsWith('llm') || s === 'vllm-ocr')
    if (!llmReady && needsLlm) {
      try {
        for (const step of steps) {
          const providerId = step === 'vllm-ocr' ? 'vllm-ocr' : 'openai-compatible'
          const prov = cfg.providers?.find((p) => p.id === providerId)
          if (prov?.model) {
            await putCurrentLlm({ target: { kind: 'provider', providerId, modelId: prov.model } })
            break // load one model per pipeline run
          }
        }
        // wait for LLM to reach ready status (poll up to 15 s)
        for (let i = 0; i < 30; i++) {
          await new Promise((r) => setTimeout(r, 500))
          const cur = await getCurrentLlm()
          if (cur?.status === 'ready') break
        }
      } catch {
        // best-effort: pipeline will fail naturally if LLM not ready
      }
    }
    try {
      const res = await startPipeline({
        steps,
        pages: [pageId],
        ...pipelineOptions(),
      })
      console.log('pipeline result:', res)
    } catch (e) {
      console.error('pipeline failed:', e)
    }
  }

  type PipelinePick = (
    p: NonNullable<Awaited<ReturnType<typeof getConfig>>['pipeline']>,
  ) => string[]
  const detectChain: PipelinePick = (p) => [
    p.detector!,
    p.segmenter!,
    p.bubble_segmenter!,
    p.font_detector!,
  ]
  const ocrChain: PipelinePick = (p) => [p.ocr!]
  const translateChain: PipelinePick = (p) => [p.translator!]
  const inpaintChain: PipelinePick = (p) => [p.inpainter!]
  const renderChain: PipelinePick = (p) => [p.renderer!]

  const isDetecting = currentStep === 'detect'
  const isOcr = currentStep === 'ocr'
  const isInpainting = currentStep === 'inpaint'
  const isTranslating = currentStep === 'llmGenerate'
  const isRendering = currentStep === 'render'

  return (
    <div className='flex items-center gap-0.5'>
      <Button
        variant='ghost'
        size='xs'
        onClick={() => void runStep(detectChain)}
        data-testid='toolbar-detect'
        disabled={!hasPage || isProcessing}
      >
        {isDetecting ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <ScanIcon className='size-4' />
        )}
        {t('processing.detect')}
      </Button>
      <Separator orientation='vertical' className='mx-0.5 h-4' />
      <Button
        variant='ghost'
        size='xs'
        onClick={() => void runStep(ocrChain)}
        data-testid='toolbar-ocr'
        disabled={!hasPage || isProcessing}
      >
        {isOcr ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <ScanTextIcon className='size-4' />
        )}
        {t('processing.ocr')}
      </Button>
      <Separator orientation='vertical' className='mx-0.5 h-4' />
      <Button
        variant='ghost'
        size='xs'
        onClick={() => void runStep(translateChain)}
        disabled={!hasPage || isProcessing}
        data-testid='toolbar-translate'
      >
        {isTranslating ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <LanguagesIcon className='size-4' />
        )}
        {t('llm.generate')}
      </Button>
      <Separator orientation='vertical' className='mx-0.5 h-4' />
      <Button
        variant='ghost'
        size='xs'
        onClick={() => void runStep(inpaintChain)}
        data-testid='toolbar-inpaint'
        disabled={!hasPage || isProcessing}
      >
        {isInpainting ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <Wand2Icon className='size-4' />
        )}
        {t('mask.inpaint')}
      </Button>
      <Separator orientation='vertical' className='mx-0.5 h-4' />
      <Button
        variant='ghost'
        size='xs'
        onClick={() => void runStep(renderChain)}
        data-testid='toolbar-render'
        disabled={!hasPage || isProcessing}
      >
        {isRendering ? (
          <LoaderCircleIcon className='size-4 animate-spin' />
        ) : (
          <TypeIcon className='size-4' />
        )}
        {t('llm.render')}
      </Button>
    </div>
  )
}

