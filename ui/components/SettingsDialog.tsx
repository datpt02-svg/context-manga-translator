'use client'

import { useQueryClient } from '@tanstack/react-query'
import {
  SunIcon,
  MoonIcon,
  MonitorIcon,
  CheckCircleIcon,
  AlertCircleIcon,
  LoaderIcon,
  PaletteIcon,
  HardDriveIcon,
  InfoIcon,
  CpuIcon,
  KeyboardIcon,
  SaveIcon,
  RotateCcwIcon,
  AlertTriangleIcon,
  CopyIcon,
  ExternalLinkIcon,
  LogInIcon,
  LogOutIcon,
  SparklesIcon,
  ChevronDownIcon,
} from 'lucide-react'
import { useTheme } from 'next-themes'
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Kbd } from '@/components/ui/kbd'
import { Label } from '@/components/ui/label'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Slider } from '@/components/ui/slider'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useUpdater, type UpdaterStatus } from '@/components/Updater'
import {
  getCatalog as getLlmCatalog,
  getConfig,
  getEngineCatalog,
  getGetCatalogQueryKey as getGetLlmCatalogQueryKey,
  getMeta,
  patchConfig,
  deleteCodexSession,
  getGetCodexAuthStatusQueryKey,
  startCodexDeviceLogin,
  useGetCodexAuthStatus,
} from '@/lib/api/default/default'
import type {
  AppConfig,
  ConfigPatch,
  CodexDeviceLogin,
  EngineCatalog as GetEngineCatalog200,
  ProviderConfig,
} from '@/lib/api/schemas'
import { isTauri, openExternalUrl } from '@/lib/backend'
import { supportedLanguages } from '@/lib/i18n'
import {
  areShortcutsEqual,
  formatShortcut,
  formatModifierCombination,
  getPlatform,
  isKeyBlocked,
  isModifierKey,
} from '@/lib/shortcutUtils'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'

// Dialog state models `AppConfig` (what `GET /config` returns — snake_case).
// But `PATCH /config` expects a `ConfigPatch` with camelCase fields because
// the patch schema derives `rename_all = "camelCase"` serde attrs. Translate
// at the boundary so the dialog internals stay unified.
type UpdateConfigBody = AppConfig

function appConfigToPatch(cfg: AppConfig): ConfigPatch {
  const patch: ConfigPatch = {}
  if (cfg.data?.path) {
    patch.data = { path: cfg.data.path }
  }
  if (cfg.http) {
    patch.http = {
      connectTimeout: cfg.http.connect_timeout,
      readTimeout: cfg.http.read_timeout,
      maxRetries: cfg.http.max_retries,
    }
  }
  if (cfg.pipeline) {
    patch.pipeline = {
      detector: cfg.pipeline.detector,
      fontDetector: cfg.pipeline.font_detector,
      segmenter: cfg.pipeline.segmenter,
      bubbleSegmenter: cfg.pipeline.bubble_segmenter,
      ocr: cfg.pipeline.ocr,
      translator: cfg.pipeline.translator,
      inpainter: cfg.pipeline.inpainter,
      renderer: cfg.pipeline.renderer,
      unlimitedOcrMode: cfg.pipeline.unlimited_ocr_mode ?? null,
      unlimitedOcrUrl: cfg.pipeline.unlimited_ocr_url ?? null,
      anytext2Url: cfg.pipeline.anytext2_url ?? null,
      detectorConfidenceThreshold: cfg.pipeline.detector_confidence_threshold ?? null,
      segmenterBinaryThreshold: cfg.pipeline.segmenter_binary_threshold ?? null,
      comicTextBubbleDetectorClasses:
        cfg.pipeline.comic_text_bubble_detector_classes ?? [
          ...DEFAULT_COMIC_TEXT_BUBBLE_DETECTOR_CLASSES,
        ],
    }
  }
  if (cfg.providers) {
    patch.providers = cfg.providers.map((p) => ({
      id: p.id,
      baseUrl: p.base_url ?? null,
      model: p.model ?? null,
      apiKey: p.api_key ?? null,
      maxTokens: p.max_tokens ?? null,
      temperature: p.temperature ?? null,
    }))
  }
  return patch
}

async function updateConfig(next: UpdateConfigBody): Promise<AppConfig> {
  return (await patchConfig(appConfigToPatch(next))) as AppConfig
}

const GITHUB_REPO = 'mayocream/koharu'

const TABS = [
  { id: 'appearance', icon: PaletteIcon, labelKey: 'settings.appearance' },
  { id: 'models', icon: SparklesIcon, labelKey: 'settings.models' },
  { id: 'engines', icon: CpuIcon, labelKey: 'settings.engines' },
  { id: 'keybinds', icon: KeyboardIcon, labelKey: 'settings.keybinds' },
  { id: 'runtime', icon: HardDriveIcon, labelKey: 'settings.runtime' },
  { id: 'about', icon: InfoIcon, labelKey: 'settings.about' },
] as const

export type TabId = (typeof TABS)[number]['id']

type SettingsDialogProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  defaultTab?: TabId
}

const DEFAULT_HTTP_CONNECT_TIMEOUT = 20
const DEFAULT_HTTP_READ_TIMEOUT = 300
const DEFAULT_HTTP_MAX_RETRIES = 3

export function SettingsDialog({
  open,
  onOpenChange,
  defaultTab = 'appearance',
}: SettingsDialogProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [tab, setTab] = useState<TabId>(defaultTab)
  useEffect(() => {
    if (open) setTab(defaultTab)
  }, [defaultTab, open])

  const [appConfig, setAppConfig] = useState<UpdateConfigBody | null>(null)
  const [apiKeyDrafts, setApiKeyDrafts] = useState<Record<string, string>>({})
  const [dataPathDraft, setDataPathDraft] = useState('')
  const [httpConnectTimeoutDraft, setHttpConnectTimeoutDraft] = useState('')
  const [httpReadTimeoutDraft, setHttpReadTimeoutDraft] = useState('')
  const [httpMaxRetriesDraft, setHttpMaxRetriesDraft] = useState('')
  const [storageSettingsError, setStorageSettingsError] = useState<string | null>(null)
  const [isSavingStorageSettings, setIsSavingStorageSettings] = useState(false)
  const [engineCatalog, setEngineCatalog] = useState<GetEngineCatalog200 | null>(null)
  const [appVersion, setAppVersion] = useState<string>()
  const updater = useUpdater()

  useEffect(() => {
    if (!open) return
    void (async () => {
      try {
        const [config, engines] = await Promise.all([
          getConfig(),
          getEngineCatalog(),
        ])
        setAppConfig(config)
        setEngineCatalog(engines)
      } catch {}
    })()
  }, [open])

  useEffect(() => {
    if (!open) return
    let cancelled = false
    void (async () => {
      try {
        const meta = await getMeta()
        if (cancelled) return
        setAppVersion(meta.version)
      } catch {
        return
      }
    })()
    return () => {
      cancelled = true
    }
  }, [open])

  const checkForUpdates = updater.checkForUpdates
  useEffect(() => {
    if (!open || !isTauri()) return
    void checkForUpdates()
  }, [open, checkForUpdates])

  useEffect(() => {
    if (!appConfig?.data) return
    setDataPathDraft(appConfig.data.path)
    setHttpConnectTimeoutDraft(
      String(appConfig.http?.connect_timeout ?? DEFAULT_HTTP_CONNECT_TIMEOUT),
    )
    setHttpReadTimeoutDraft(String(appConfig.http?.read_timeout ?? DEFAULT_HTTP_READ_TIMEOUT))
    setHttpMaxRetriesDraft(String(appConfig.http?.max_retries ?? DEFAULT_HTTP_MAX_RETRIES))
    setStorageSettingsError(null)
  }, [appConfig])

  const persistConfig = async (next: UpdateConfigBody) => {
    try {
      const saved = await updateConfig(next)
      setAppConfig(saved)
      queryClient.invalidateQueries({ queryKey: getGetLlmCatalogQueryKey() })
      return saved
    } catch {
      return null
    }
  }

  const upsertProvider = (id: string, updater: (p: ProviderConfig) => ProviderConfig) => {
    if (!appConfig) return appConfig
    const providers = [...(appConfig.providers ?? [])]
    const idx = providers.findIndex((p) => p.id === id)
    const current = idx >= 0 ? providers[idx] : { id }
    if (idx >= 0) providers[idx] = updater(current)
    else providers.push(updater(current))
    const next = { ...appConfig, providers }
    setAppConfig(next)
    return next
  }

  const handleApplyStorageSettings = async () => {
    if (!appConfig) return
    const path = dataPathDraft.trim()
    if (!path) {
      setStorageSettingsError('Required')
      return
    }
    const connectTimeout = Number.parseInt(httpConnectTimeoutDraft.trim(), 10)
    if (!Number.isInteger(connectTimeout) || connectTimeout <= 0) {
      setStorageSettingsError('Invalid HTTP connect timeout')
      return
    }
    const readTimeout = Number.parseInt(httpReadTimeoutDraft.trim(), 10)
    if (!Number.isInteger(readTimeout) || readTimeout <= 0) {
      setStorageSettingsError('Invalid HTTP read timeout')
      return
    }
    const maxRetries = Number.parseInt(httpMaxRetriesDraft.trim(), 10)
    if (!Number.isInteger(maxRetries) || maxRetries < 0) {
      setStorageSettingsError('Invalid HTTP max retries')
      return
    }

    setIsSavingStorageSettings(true)
    setStorageSettingsError(null)
    const saved = await persistConfig({
      ...appConfig,
      data: { path },
      http: {
        connect_timeout: connectTimeout,
        read_timeout: readTimeout,
        max_retries: maxRetries,
      },
    })
    setIsSavingStorageSettings(false)
    if (!saved) {
      setStorageSettingsError('Failed')
      return
    }
    if (!isTauri()) {
      setStorageSettingsError('Restart manually')
      return
    }
    try {
      const { relaunch } = await import('@tauri-apps/plugin-process')
      await relaunch()
    } catch {
      setStorageSettingsError('Restart manually')
    }
  }

  const storageSettingsUnchanged =
    dataPathDraft.trim() === appConfig?.data?.path &&
    httpConnectTimeoutDraft.trim() ===
      String(appConfig?.http?.connect_timeout ?? DEFAULT_HTTP_CONNECT_TIMEOUT) &&
    httpReadTimeoutDraft.trim() ===
      String(appConfig?.http?.read_timeout ?? DEFAULT_HTTP_READ_TIMEOUT) &&
    httpMaxRetriesDraft.trim() === String(appConfig?.http?.max_retries ?? DEFAULT_HTTP_MAX_RETRIES)

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='flex h-[600px] max-h-[85vh] w-[760px] max-w-[92vw] flex-col gap-0 overflow-hidden p-0'>
        <DialogTitle className='sr-only'>{t('settings.title')}</DialogTitle>
        <DialogDescription className='sr-only'>Settings</DialogDescription>

        <div className='flex h-full'>
          {/* Sidebar */}
          <nav className='flex w-[180px] shrink-0 flex-col gap-1 border-r border-border bg-muted/30 p-3'>
            <p className='mb-3 px-3 text-[10px] font-semibold tracking-widest text-muted-foreground uppercase'>
              {t('settings.title')}
            </p>
            {TABS.map(({ id, icon: Icon, labelKey }) => (
              <button
                key={id}
                onClick={() => setTab(id)}
                data-active={tab === id}
                className='flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-muted-foreground transition hover:text-foreground data-[active=true]:bg-accent data-[active=true]:text-accent-foreground'
              >
                <Icon className='size-4 shrink-0' />
                {t(labelKey)}
              </button>
            ))}
          </nav>

          {/* Content */}
          <ScrollArea className='min-h-0 flex-1'>
            <div className='p-6'>
              {tab === 'appearance' && <AppearancePane />}
              {tab === 'engines' && engineCatalog && appConfig && (
                <EnginesPane
                  catalog={engineCatalog}
                  pipeline={appConfig.pipeline ?? {}}
                  onChange={(pipeline) => {
                    const next = { ...appConfig, pipeline }
                    setAppConfig(next)
                    void persistConfig(next)
                  }}
                />
              )}
              {tab === 'models' && appConfig && (
                <ModelsPane
                  config={appConfig}
                  drafts={apiKeyDrafts}
                  onBaseUrlChange={(id, v) => {
                    const next = upsertProvider(id, (p) => ({
                      ...p,
                      base_url: v || null,
                    }))
                    if (next) void persistConfig(next)
                  }}
                  onBaseUrlBlur={() => {}}
                  onModelChange={(id, v) => {
                    const next = upsertProvider(id, (p) => ({ ...p, model: v || null }))
                    if (next) void persistConfig(next)
                  }}
                  onMaxTokensChange={(id, v) => {
                    const next = upsertProvider(id, (p) => ({ ...p, max_tokens: v ? Number(v) : null }))
                    if (next) void persistConfig(next)
                  }}
                  onTemperatureChange={(id, v) => {
                    const next = upsertProvider(id, (p) => ({ ...p, temperature: v ? Number(v) : null }))
                    if (next) void persistConfig(next)
                  }}
                  onApiKeyChange={(id, v) => setApiKeyDrafts((c) => ({ ...c, [id]: v }))}
                  onSaveKey={(id) => {
                    const key = apiKeyDrafts[id]?.trim()
                    if (!key || !appConfig) return
                    const providers = [...(appConfig.providers ?? [])]
                    const idx = providers.findIndex((p) => p.id === id)
                    const current = idx >= 0 ? providers[idx] : { id }
                    const updated = { ...current, api_key: key }
                    if (idx >= 0) providers[idx] = updated
                    else providers.push(updated)
                    void persistConfig({ ...appConfig, providers }).then(() =>
                      setApiKeyDrafts((c) => {
                        const n = { ...c }
                        delete n[id]
                        return n
                      }),
                    )
                  }}
                  onClearKey={(id) => {
                    if (!appConfig) return
                    const providers = [...(appConfig.providers ?? [])]
                    const idx = providers.findIndex((p) => p.id === id)
                    if (idx >= 0) providers[idx] = { ...providers[idx], api_key: null }
                    void persistConfig({ ...appConfig, providers }).then(() =>
                      setApiKeyDrafts((c) => {
                        const n = { ...c }
                        delete n[id]
                        return n
                      }),
                    )
                  }}
                />
              )}
              {tab === 'ai' && <CodexSettingsPane />}
              {tab === 'runtime' && (
                <StoragePane
                  dataPath={dataPathDraft}
                  httpConnectTimeout={httpConnectTimeoutDraft}
                  httpReadTimeout={httpReadTimeoutDraft}
                  httpMaxRetries={httpMaxRetriesDraft}
                  error={storageSettingsError}
                  saving={isSavingStorageSettings}
                  unchanged={storageSettingsUnchanged}
                  onPathChange={(v) => {
                    setDataPathDraft(v)
                    setStorageSettingsError(null)
                  }}
                  onHttpConnectTimeoutChange={(v) => {
                    setHttpConnectTimeoutDraft(v)
                    setStorageSettingsError(null)
                  }}
                  onHttpReadTimeoutChange={(v) => {
                    setHttpReadTimeoutDraft(v)
                    setStorageSettingsError(null)
                  }}
                  onHttpMaxRetriesChange={(v) => {
                    setHttpMaxRetriesDraft(v)
                    setStorageSettingsError(null)
                  }}
                  onApply={() => void handleApplyStorageSettings()}
                />
              )}
              {tab === 'keybinds' && <KeybindsPane />}
              {tab === 'about' && (
                <AboutPane
                  version={appVersion}
                  latestVersion={updater.latestVersion}
                  status={updater.status}
                  isInstallingUpdate={updater.isInstalling}
                  onInstallUpdate={() => void updater.installUpdate()}
                />
              )}
            </div>
          </ScrollArea>
        </div>
      </DialogContent>
    </Dialog>
  )
}

// ── Appearance ────────────────────────────────────────────────────

const THEMES = [
  { value: 'light', icon: SunIcon, labelKey: 'settings.themeLight' },
  { value: 'dark', icon: MoonIcon, labelKey: 'settings.themeDark' },
  { value: 'system', icon: MonitorIcon, labelKey: 'settings.themeSystem' },
] as const

function AppearancePane() {
  const { t, i18n } = useTranslation()
  const { theme, setTheme } = useTheme()
  const locales = useMemo(() => supportedLanguages, [])
  return (
    <div className='space-y-8'>
      <Section title={t('settings.theme')}>
        <div className='grid grid-cols-3 gap-3'>
          {THEMES.map(({ value, icon: Icon, labelKey }) => (
            <button
              key={value}
              onClick={() => setTheme(value)}
              data-active={theme === value}
              className='flex flex-col items-center gap-2 rounded-xl border border-border bg-card px-4 py-4 text-muted-foreground transition hover:border-foreground/30 data-[active=true]:border-primary data-[active=true]:text-foreground'
            >
              <Icon className='size-5' />
              <span className='text-xs font-medium'>{t(labelKey)}</span>
            </button>
          ))}
        </div>
      </Section>

      <Section title={t('settings.language')}>
        <Select value={i18n.language} onValueChange={(v) => i18n.changeLanguage(v)}>
          <SelectTrigger className='w-full'>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {locales.map((code) => (
              <SelectItem key={code} value={code}>
                {t(`menu.languages.${code}`, { defaultValue: code })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Section>
    </div>
  )
}

// ── Engines ──────────────────────────────────────────────────────

// Client-side display-only defaults, mirroring each detector's built-in
// threshold in Rust (koharu-ml). Not the source of truth — only used to
// show a "Default: X" hint and to seed the slider when no override is set.
const DETECTOR_DEFAULT_THRESHOLD: Record<string, number> = {
  'comic-text-bubble-detector': 0.3,
  'comic-text-detector': 0.4,
  'pp-doclayout-v3': 0.25,
  'anime-text': 0.25,
}
const SEGMENTER_DEFAULT_THRESHOLD: Record<string, number> = {
  'comic-text-detector-seg': 60 / 255,
}

const DEFAULT_COMIC_TEXT_BUBBLE_DETECTOR_CLASSES = ['text_bubble', 'text_free'] as const
const COMIC_TEXT_BUBBLE_DETECTOR_CLASSES = [
  'bubble',
  ...DEFAULT_COMIC_TEXT_BUBBLE_DETECTOR_CLASSES,
] as const

export function EnginesPane({
  catalog,
  pipeline,
  onChange,
}: {
  catalog: GetEngineCatalog200
  pipeline: import('@/lib/api/schemas').PipelineConfig
  onChange: (pipeline: import('@/lib/api/schemas').PipelineConfig) => void
}) {
  const { t } = useTranslation()

  const currentDetector = pipeline.detector ?? catalog.detectors[0]?.id ?? ''
  const detectorDefaultThreshold = DETECTOR_DEFAULT_THRESHOLD[currentDetector]
  const detectorThreshold = pipeline.detector_confidence_threshold ?? detectorDefaultThreshold
  const currentSegmenter = pipeline.segmenter ?? catalog.segmenters[0]?.id ?? ''
  const segmenterDefaultThreshold = SEGMENTER_DEFAULT_THRESHOLD[currentSegmenter]
  const segmenterThreshold = pipeline.segmenter_binary_threshold ?? segmenterDefaultThreshold
  const comicTextBubbleDetectorClasses =
    pipeline.comic_text_bubble_detector_classes ?? [...DEFAULT_COMIC_TEXT_BUBBLE_DETECTOR_CLASSES]

  // Local draft values so dragging sliders feels smooth (no network
  // round-trip per pixel). Only persisted via onValueCommit on release.
  const [thresholdDraft, setThresholdDraft] = useState(detectorThreshold)
  const [segmenterThresholdDraft, setSegmenterThresholdDraft] = useState(segmenterThreshold)
  useEffect(() => {
    setThresholdDraft(detectorThreshold)
  }, [detectorThreshold])
  useEffect(() => {
    setSegmenterThresholdDraft(segmenterThreshold)
  }, [segmenterThreshold])

  const sections = [
    {
      label: t('settings.detector'),
      key: 'detector' as const,
      engines: catalog.detectors,
    },
    {
      label: t('settings.fontDetector'),
      key: 'font_detector' as const,
      engines: catalog.fontDetectors,
    },
    {
      label: t('settings.segmenter'),
      key: 'segmenter' as const,
      engines: catalog.segmenters,
    },
    {
      label: t('settings.bubbleSegmenter'),
      key: 'bubble_segmenter' as const,
      engines: catalog.bubbleSegmenters,
    },
    { label: t('settings.ocr'), key: 'ocr' as const, engines: catalog.ocr },
    {
      label: t('settings.translator'),
      key: 'translator' as const,
      engines: catalog.translators,
    },
    {
      label: t('settings.inpainter'),
      key: 'inpainter' as const,
      engines: catalog.inpainters,
    },
    {
      label: t('settings.renderer'),
      key: 'renderer' as const,
      engines: catalog.renderers,
    },
  ]

  return (
    <>
      <div className='space-y-4'>
        <p className='text-xs text-muted-foreground'>{t('settings.enginesDescription')}</p>
        {sections.map(({ label, key, engines }) => (
          <div key={key} className='space-y-1.5'>
            <Label className='text-xs'>{label}</Label>
            <Select
              value={pipeline[key] ?? engines[0]?.id ?? ''}
              onValueChange={(v) => onChange({ ...pipeline, [key]: v })}
            >
              <SelectTrigger className='w-full'>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {engines.map((e) => (
                  <SelectItem key={e.id} value={e.id}>
                    {e.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {key === 'detector' && detectorDefaultThreshold !== undefined && (
              <div className='space-y-1.5 pt-1 pl-1'>
                <div className='flex items-center justify-between'>
                  <Label className='text-xs text-muted-foreground'>
                    {t('settings.detectorConfidenceThreshold')}
                  </Label>
                  <div className='flex items-center gap-2'>
                    <span className='text-xs tabular-nums text-muted-foreground'>
                      {thresholdDraft?.toFixed(2)}
                    </span>
                    {pipeline.detector_confidence_threshold != null && (
                      <button
                        type='button'
                        className='text-xs text-muted-foreground underline hover:text-foreground'
                        onClick={() =>
                          onChange({ ...pipeline, detector_confidence_threshold: null })
                        }
                      >
                        {t('settings.detectorConfidenceThresholdReset')}
                      </button>
                    )}
                  </div>
                </div>
                <Slider
                  value={[thresholdDraft ?? detectorDefaultThreshold]}
                  min={0.05}
                  max={0.95}
                  step={0.01}
                  onValueChange={([v]) => setThresholdDraft(v)}
                  onValueCommit={([v]) =>
                    onChange({ ...pipeline, detector_confidence_threshold: v })
                  }
                />
                <p className='text-xs text-muted-foreground'>
                  {t('settings.detectorConfidenceThresholdDescription')}{' '}
                  {t('settings.detectorConfidenceThresholdDefault', {
                    value: detectorDefaultThreshold.toFixed(2),
                  })}
                </p>
              </div>
            )}
            {key === 'segmenter' && segmenterDefaultThreshold !== undefined && (
              <div className='space-y-1.5 pt-1 pl-1'>
                <div className='flex items-center justify-between'>
                  <Label className='text-xs text-muted-foreground'>
                    {t('settings.segmenterBinaryThreshold')}
                  </Label>
                  <div className='flex items-center gap-2'>
                    <span className='text-xs tabular-nums text-muted-foreground'>
                      {segmenterThresholdDraft?.toFixed(2)}
                    </span>
                    {pipeline.segmenter_binary_threshold != null && (
                      <button
                        type='button'
                        className='text-xs text-muted-foreground underline hover:text-foreground'
                        onClick={() => onChange({ ...pipeline, segmenter_binary_threshold: null })}
                      >
                        {t('settings.segmenterBinaryThresholdReset')}
                      </button>
                    )}
                  </div>
                </div>
                <Slider
                  value={[segmenterThresholdDraft ?? segmenterDefaultThreshold]}
                  min={0}
                  max={1}
                  step={0.01}
                  onValueChange={([v]) => setSegmenterThresholdDraft(v)}
                  onValueCommit={([v]) => onChange({ ...pipeline, segmenter_binary_threshold: v })}
                />
                <p className='text-xs text-muted-foreground'>
                  {t('settings.segmenterBinaryThresholdDescription')}{' '}
                  {t('settings.segmenterBinaryThresholdDefault', {
                    value: segmenterDefaultThreshold.toFixed(2),
                  })}
                </p>
              </div>
            )}
            {key === 'detector' && currentDetector === 'comic-text-bubble-detector' && (
              <div className='space-y-1.5 pt-1 pl-1'>
                <Label className='text-xs text-muted-foreground'>
                  {t('settings.comicTextBubbleDetectorClasses')}
                </Label>
                <Popover>
                  <PopoverTrigger asChild>
                    <Button
                      type='button'
                      variant='outline'
                      className='h-9 w-full justify-between text-left text-xs font-normal'
                    >
                      <span className='truncate'>
                        {comicTextBubbleDetectorClasses
                          .map((className) =>
                            t(`settings.comicTextBubbleDetectorClass.${className}`),
                          )
                          .join(', ')}
                      </span>
                      <ChevronDownIcon className='size-4 shrink-0 opacity-50' />
                    </Button>
                  </PopoverTrigger>
                  <PopoverContent
                    align='start'
                    className='min-w-(--radix-popover-trigger-width) p-1'
                  >
                    {COMIC_TEXT_BUBBLE_DETECTOR_CLASSES.map((className) => {
                      const checked = comicTextBubbleDetectorClasses.includes(className)
                      const disabled = checked && comicTextBubbleDetectorClasses.length === 1
                      return (
                        <label
                          key={className}
                          className={`flex items-center gap-2 rounded-md px-2 py-1.5 text-sm ${
                            disabled
                              ? 'cursor-not-allowed opacity-50'
                              : 'cursor-pointer hover:bg-accent'
                          }`}
                        >
                          <input
                            type='checkbox'
                            className='size-4 accent-primary'
                            checked={checked}
                            disabled={disabled}
                            onChange={() => {
                              const nextClasses = checked
                                ? comicTextBubbleDetectorClasses.filter((item) => item !== className)
                                : [...comicTextBubbleDetectorClasses, className]
                              if (nextClasses.length) {
                                onChange({
                                  ...pipeline,
                                  comic_text_bubble_detector_classes: nextClasses,
                                })
                              }
                            }}
                          />
                          <span>{t(`settings.comicTextBubbleDetectorClass.${className}`)}</span>
                        </label>
                      )
                    })}
                  </PopoverContent>
                </Popover>
                <p className='text-xs text-muted-foreground'>
                  {t('settings.comicTextBubbleDetectorClassesDescription')}
                </p>
              </div>
            )}
          </div>
        ))}
      </div>

      {/* ── Unlimited-OCR mode ── */}
      <div className='space-y-3 pt-4'>
        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('settings.unlimitedOcrMode')}</Label>
          <Select
            value={pipeline.unlimited_ocr_mode ?? 'off'}
            onValueChange={(v) => onChange({ ...pipeline, unlimited_ocr_mode: v as 'off' | 'smart-fallback' | 'full' })}
          >
            <SelectTrigger className='w-full'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='off'>Off</SelectItem>
              <SelectItem value='smart-fallback'>Smart fallback</SelectItem>
              <SelectItem value='full'>Full Unlimited-OCR</SelectItem>
            </SelectContent>
          </Select>
        </div>
        {pipeline.unlimited_ocr_mode !== 'off' && (
          <div className='space-y-1.5'>
            <Label className='text-xs'>{t('settings.unlimitedOcrUrl')}</Label>
            <Input
              value={pipeline.unlimited_ocr_url ?? 'http://127.0.0.1:7862'}
              onChange={(e) => onChange({ ...pipeline, unlimited_ocr_url: e.target.value || null })}
              placeholder='http://127.0.0.1:7862'
            />
          </div>
        )}
      </div>
      {/* ── AnyText2 renderer ── */}
      <div className='space-y-3 pt-2'>
        <div className='flex items-center justify-between'>
          <Label className='text-xs'>{t('settings.anytext2')}</Label>
          <Switch
            checked={pipeline.renderer === 'anytext2'}
            onCheckedChange={(v) =>
              onChange({ ...pipeline, renderer: v ? 'anytext2' : 'koharu-renderer' })
            }
          />
        </div>
        {pipeline.renderer === 'anytext2' && (
          <div className='space-y-1.5'>
            <Label className='text-xs'>{t('settings.anytext2Url')}</Label>
            <Input
              value={pipeline.anytext2_url ?? 'http://127.0.0.1:7863'}
              onChange={(e) => onChange({ ...pipeline, anytext2_url: e.target.value || null })}
              placeholder='http://127.0.0.1:7863'
            />
          </div>
        )}
      </div>
    </>
  )
}

function ModelsPane({
  config,
  drafts,
  onBaseUrlChange,
  onBaseUrlBlur,
  onModelChange,
  onMaxTokensChange,
  onTemperatureChange,
  onApiKeyChange,
  onSaveKey,
  onClearKey,
}: {
  config: UpdateConfigBody
  drafts: Record<string, string>
  onBaseUrlChange: (id: string, v: string) => void
  onBaseUrlBlur: () => void
  onModelChange: (id: string, v: string) => void
  onMaxTokensChange: (id: string, v: string) => void
  onTemperatureChange: (id: string, v: string) => void
  onApiKeyChange: (id: string, v: string) => void
  onSaveKey: (id: string) => void
  onClearKey: (id: string) => void
}) {
  const { t } = useTranslation()
  const customSystemPrompt = usePreferencesStore((s) => s.customSystemPrompt)
  const setCustomSystemPrompt = usePreferencesStore((s) => s.setCustomSystemPrompt)
  const targetLanguage = usePreferencesStore((s) => s.targetLanguage)
  const setTargetLanguage = usePreferencesStore((s) => s.setTargetLanguage)
  const vllmSystemPrompt = usePreferencesStore((s) => s.vllmSystemPrompt)
  const setVllmSystemPrompt = usePreferencesStore((s) => s.setVllmSystemPrompt)
  const vllmTargetLanguage = usePreferencesStore((s) => s.vllmTargetLanguage)
  const setVllmTargetLanguage = usePreferencesStore((s) => s.setVllmTargetLanguage)

  return (
    <div className='space-y-8'>
      {/* ── LLM (Translate) ── */}
      <div className='space-y-4 rounded-lg border p-4'>
        <div className='space-y-1'>
          <h3 className='text-sm font-medium'>{t('settings.llmSettings')}</h3>
          <p className='text-xs text-muted-foreground'>{t('settings.modelsDescription')}</p>
        </div>
        <ProviderSettingsCard
          id='openai-compatible'
          config={config}
          baseUrlPlaceholder='http://127.0.0.1:8000/v1'
          showTemperature
          modelPlaceholder='e.g. qwen2.5-14b-instruct'
          drafts={drafts}
          onBaseUrlChange={onBaseUrlChange}
          onBaseUrlBlur={onBaseUrlBlur}
          onModelChange={onModelChange}
          onMaxTokensChange={onMaxTokensChange}
          onTemperatureChange={onTemperatureChange}
          onApiKeyChange={onApiKeyChange}
          onSaveKey={onSaveKey}
          onClearKey={onClearKey}
        />
        <div className='space-y-3'>
          <div className='space-y-1.5'>
            <Label>{t('settings.targetLanguage')}</Label>
            <Select value={targetLanguage ?? 'vi-VN'} onValueChange={(v) => { setTargetLanguage(v); if (!vllmTargetLanguage) setVllmTargetLanguage(v); }}>
              <SelectTrigger data-testid='settings-language-select' className='w-full'>
                <SelectValue placeholder={t('settings.targetLanguagePlaceholder')} />
              </SelectTrigger>
              <SelectContent position='popper'>
                {Object.entries(t('llm.languages', { returnObjects: true }) as Record<string, string>).map(([code, label]) => (
                  <SelectItem key={code} value={code}>
                    {label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className='space-y-1.5'>
            <Label>{t('settings.systemPrompt')}</Label>
            <p className='text-xs text-muted-foreground'>{t('settings.systemPromptDescription')}</p>
            <Textarea
              data-testid='settings-system-prompt'
              value={customSystemPrompt ?? ''}
              onChange={(e) => setCustomSystemPrompt(e.target.value || undefined)}
              placeholder={t('settings.systemPromptPlaceholder')}
              rows={5}
              className='min-h-0 resize-y'
            />
          </div>
        </div>
      </div>

      {/* ── vLLM (OCR) ── */}
      <div className='space-y-4 rounded-lg border p-4'>
        <div className='space-y-1'>
          <h3 className='text-sm font-medium'>{t('settings.vllmSettings')}</h3>
          <p className='text-xs text-muted-foreground'>{t('settings.modelsDescription')}</p>
        </div>
        <ProviderSettingsCard
          id='vllm-ocr'
          config={config}
          baseUrlPlaceholder='http://127.0.0.1:8000/v1'
          modelPlaceholder='e.g. qwen2.5-vl-7b-instruct'
          showTemperature
          drafts={drafts}
          onBaseUrlChange={onBaseUrlChange}
          onBaseUrlBlur={onBaseUrlBlur}
          onModelChange={onModelChange}
          onMaxTokensChange={onMaxTokensChange}
          onTemperatureChange={onTemperatureChange}
          onApiKeyChange={onApiKeyChange}
          onSaveKey={onSaveKey}
          onClearKey={onClearKey}
        />
        <div className='space-y-3'>
          <div className='space-y-1.5'>
            <Label>{t('settings.vllmTargetLanguage')}</Label>
            <Select value={vllmTargetLanguage ?? 'ja-JP'} onValueChange={setVllmTargetLanguage}>
              <SelectTrigger data-testid='settings-vllm-language-select' className='w-full'>
                <SelectValue placeholder={t('settings.targetLanguagePlaceholder')} />
              </SelectTrigger>
              <SelectContent position='popper'>
                {Object.entries(t('llm.languages', { returnObjects: true }) as Record<string, string>).map(([code, label]) => (
                  <SelectItem key={code} value={code}>
                    {label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className='space-y-1.5'>
            <Label>{t('settings.vllmSystemPrompt')}</Label>
            <p className='text-xs text-muted-foreground'>{t('settings.vllmSystemPromptDescription')}</p>
            <Textarea
              data-testid='settings-vllm-system-prompt'
              value={vllmSystemPrompt ?? ''}
              onChange={(e) => setVllmSystemPrompt(e.target.value || undefined)}
              placeholder={t('settings.vllmSystemPromptPlaceholder')}
              rows={5}
              className='min-h-0 resize-y'
            />
          </div>
        </div>
      </div>
    </div>
  )
}

/** Reusable card: base URL, model, API key, max tokens, optional temperature. */
function ProviderSettingsCard({
  id,
  title,
  description,
  config,
  baseUrlPlaceholder,
  modelPlaceholder,
  showTemperature,
  drafts,
  onBaseUrlChange,
  onBaseUrlBlur,
  onModelChange,
  onMaxTokensChange,
  onTemperatureChange,
  onApiKeyChange,
  onSaveKey,
  onClearKey,
}: {
  id: string
  title: string
  description?: string
  config: UpdateConfigBody
  baseUrlPlaceholder?: string
  modelPlaceholder?: string
  showTemperature?: boolean
  drafts: Record<string, string>
  onBaseUrlChange: (id: string, v: string) => void
  onBaseUrlBlur: () => void
  onModelChange: (id: string, v: string) => void
  onMaxTokensChange: (id: string, v: string) => void
  onTemperatureChange: (id: string, v: string) => void
  onApiKeyChange: (id: string, v: string) => void
  onSaveKey: (id: string) => void
  onClearKey: (id: string) => void
}) {
  const { t } = useTranslation()
  const cfg = config.providers?.find((p) => p.id === id)
  const draft = drafts[id] ?? ''
  const hasDraft = draft.trim().length > 0

  return (
    <Section title={title} description={description}>
      <div className='space-y-3'>
        {/* Base URL */}
        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('settings.baseUrl')}</Label>
          <Input
            type='url'
            value={cfg?.base_url ?? ''}
            onChange={(e) => onBaseUrlChange(id, e.target.value)}
            onBlur={onBaseUrlBlur}
            placeholder={baseUrlPlaceholder ?? 'https://api.example.com/v1'}
          />
        </div>

        {/* Model */}
        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('settings.model')}</Label>
          <Input
            value={cfg?.model ?? ''}
            onChange={(e) => onModelChange(id, e.target.value)}
            placeholder={modelPlaceholder ?? 'model-id'}
          />
        </div>

        {/* API key */}
        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('settings.apiKey')}</Label>
          <div className='flex gap-2'>
            <Input
              type='password'
              value={draft}
              onChange={(e) => onApiKeyChange(id, e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && hasDraft) onSaveKey(id)
              }}
              placeholder={
                cfg?.api_key === '[REDACTED]'
                  ? t('settings.apiKeyPlaceholderStored')
                  : t('settings.apiKeyPlaceholderEmpty')
              }
              className='[&::-ms-reveal]:hidden'
            />
            {hasDraft ? (
              <Button size='sm' onClick={() => onSaveKey(id)}>
                {t('settings.apiKeySave')}
              </Button>
            ) : cfg?.api_key === '[REDACTED]' ? (
              <Button variant='destructive' size='sm' onClick={() => onClearKey(id)}>
                {t('settings.apiKeyClear')}
              </Button>
            ) : null}
          </div>
        </div>

        {/* Max tokens */}
        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('settings.maxTokens')}</Label>
          <Input
            type='number'
            min={1}
            value={cfg?.max_tokens ?? ''}
            onChange={(e) => onMaxTokensChange(id, e.target.value)}
            placeholder='20000'
          />
        </div>

        {/* Temperature */}
        {(showTemperature || id === 'openai-compatible') && (
          <div className='space-y-1.5'>
            <Label className='text-xs'>{t('settings.temperature')}</Label>
            <Input
              type='number'
              min={0}
              max={2}
              step={0.1}
              value={cfg?.temperature ?? ''}
              onChange={(e) => onTemperatureChange(id, e.target.value)}
              placeholder='0.8'
            />
          </div>
        )}
      </div>
    </Section>
  )
}

// ── Keybinds ──────────────────────────────────────────────────────

function CodexSettingsPane() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [login, setLogin] = useState<CodexDeviceLogin | null>(null)
  const [loginOpen, setLoginOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [copied, setCopied] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const { data: auth, refetch } = useGetCodexAuthStatus()

  const loginStatus = auth?.login?.status
  const signedIn = auth?.signedIn === true

  useEffect(() => {
    if (!loginOpen && loginStatus !== 'pending') return
    const id = window.setInterval(() => void refetch(), 2000)
    return () => window.clearInterval(id)
  }, [loginOpen, loginStatus, refetch])

  useEffect(() => {
    if (loginOpen && (signedIn || loginStatus === 'succeeded')) {
      const id = window.setTimeout(() => setLoginOpen(false), 700)
      return () => window.clearTimeout(id)
    }
  }, [loginOpen, loginStatus, signedIn])

  const statusLabel = useMemo(() => {
    if (signedIn) return auth?.accountId ? auth.accountId : t('ai.signedIn')
    if (loginStatus === 'failed') return t('ai.signInFailed')
    if (loginStatus === 'pending') return t('ai.signInPending')
    return t('ai.signedOut')
  }, [auth?.accountId, loginStatus, signedIn, t])

  const invalidateAuth = () =>
    queryClient.invalidateQueries({ queryKey: getGetCodexAuthStatusQueryKey() })

  const handleSignIn = async () => {
    setBusy(true)
    setActionError(null)
    try {
      const next = await startCodexDeviceLogin()
      setLogin(next)
      setCopied(false)
      setLoginOpen(true)
      void invalidateAuth()
      void openExternalUrl(next.verificationUrl)
    } catch (err) {
      setActionError(String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleLogout = async () => {
    setBusy(true)
    setActionError(null)
    try {
      await deleteCodexSession()
      await invalidateAuth()
    } catch (err) {
      setActionError(String(err))
    } finally {
      setBusy(false)
    }
  }

  const handleCopyCode = async () => {
    if (!login?.userCode || typeof navigator === 'undefined') return
    await navigator.clipboard?.writeText(login.userCode)
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1200)
  }

  return (
    <Section title={t('settings.codex')} description={t('settings.codexDescription')}>
      <div className='rounded-md border border-amber-200/70 bg-amber-50/80 p-3 text-xs leading-relaxed text-amber-900 dark:border-amber-900/70 dark:bg-amber-950/40 dark:text-amber-100'>
        {t('settings.codexTwoFactorDescription')}
      </div>
      <div className='rounded-md border border-border bg-card p-3'>
        <div className='flex items-center justify-between gap-3'>
          <div className='flex min-w-0 items-center gap-2'>
            <div className='flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary'>
              <SparklesIcon className='size-4' />
            </div>
            <div className='min-w-0'>
              <div className='text-sm font-medium text-foreground'>Codex</div>
              <div className='truncate text-xs text-muted-foreground'>{statusLabel}</div>
            </div>
          </div>
          {signedIn ? (
            <Button
              variant='outline'
              size='sm'
              className='gap-1.5'
              disabled={busy}
              onClick={() => void handleLogout()}
            >
              <LogOutIcon className='size-3.5' />
              {t('ai.signOut')}
            </Button>
          ) : (
            <Button
              variant='default'
              size='sm'
              className='gap-1.5'
              disabled={busy}
              onClick={() => void handleSignIn()}
            >
              {busy ? (
                <LoaderIcon className='size-3.5 animate-spin' />
              ) : (
                <LogInIcon className='size-3.5' />
              )}
              {t('ai.signIn')}
            </Button>
          )}
        </div>
        {(actionError || (auth?.login?.status === 'failed' && auth.login.error)) && (
          <p className='mt-2 line-clamp-3 text-xs text-destructive'>
            {actionError || auth?.login?.error}
          </p>
        )}
      </div>

      <Dialog open={loginOpen} onOpenChange={setLoginOpen}>
        <DialogContent className='w-[340px] max-w-[92vw] gap-3 p-4'>
          <DialogTitle className='text-sm'>{t('ai.signInTitle')}</DialogTitle>
          <DialogDescription className='sr-only'>{t('ai.signIn')}</DialogDescription>
          <div className='flex items-center justify-between gap-3 rounded-md border border-border bg-muted/40 px-3 py-2'>
            <div className='min-w-0'>
              <div className='text-[10px] font-semibold tracking-wide text-muted-foreground uppercase'>
                {t('ai.userCode')}
              </div>
              <div className='mt-0.5 font-mono text-xl font-semibold tracking-widest'>
                {login?.userCode ?? '...'}
              </div>
            </div>
            <Button
              variant='outline'
              size='icon-sm'
              disabled={!login}
              aria-label={copied ? t('common.copied') : t('common.copy')}
              onClick={() => void handleCopyCode()}
            >
              <CopyIcon className='size-3.5' />
            </Button>
          </div>
          <Button
            variant='outline'
            size='sm'
            className='w-full gap-1.5'
            disabled={!login}
            onClick={() => login && void openExternalUrl(login.verificationUrl)}
          >
            <ExternalLinkIcon className='size-3.5' />
            {t('ai.openBrowser')}
          </Button>
          <div className='flex items-center gap-2 text-xs text-muted-foreground'>
            {signedIn || loginStatus === 'succeeded' ? (
              <>
                <CheckCircleIcon className='size-4 text-green-500' />
                {t('ai.signInComplete')}
              </>
            ) : loginStatus === 'failed' ? (
              <>
                <AlertCircleIcon className='size-4 text-destructive' />
                <span className='line-clamp-2'>{auth?.login?.error ?? t('ai.signInFailed')}</span>
              </>
            ) : (
              <>
                <LoaderIcon className='size-4 animate-spin' />
                {t('ai.signInPending')}
              </>
            )}
          </div>
        </DialogContent>
      </Dialog>
    </Section>
  )
}

const SHORTCUT_ITEMS = [
  { key: 'select', labelKey: 'toolRail.select' },
  { key: 'block', labelKey: 'toolRail.block' },
  { key: 'brush', labelKey: 'toolRail.brush' },
  { key: 'eraser', labelKey: 'toolRail.eraser' },
  { key: 'repairBrush', labelKey: 'toolRail.repairBrush' },
  {
    key: 'increaseBrushSize',
    labelKey: 'settings.shortcutIncreaseBrushSize',
  },
  {
    key: 'decreaseBrushSize',
    labelKey: 'settings.shortcutDecreaseBrushSize',
  },
  { key: 'undo', labelKey: 'menu.undo' },
  { key: 'redo', labelKey: 'menu.redo' },
] as const

function KeybindsPane() {
  const { t } = useTranslation()
  const shortcuts = usePreferencesStore((state) => state.shortcuts)
  const setShortcuts = usePreferencesStore((state) => state.setShortcuts)
  const resetShortcutsStore = usePreferencesStore((state) => state.resetShortcuts)
  const [pendingShortcuts, setPendingShortcuts] = useState(shortcuts)
  const [recordingKey, setRecordingKey] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [isSaved, setIsSaved] = useState(false)
  const [liveShortcut, setLiveShortcut] = useState<string | null>(null)
  const isMac = useMemo(() => getPlatform() === 'mac', [])

  // Optimized conflict detection
  const conflictCounts = useMemo(() => {
    const counts = new Map<string, number>()
    Object.values(pendingShortcuts).forEach((val) => {
      counts.set(val, (counts.get(val) || 0) + 1)
    })
    return counts
  }, [pendingShortcuts])

  const isDirty = useMemo(
    () => !areShortcutsEqual(shortcuts, pendingShortcuts),
    [shortcuts, pendingShortcuts],
  )

  // Sync from store if it changes (e.g. externally via Reset)
  useEffect(() => {
    setPendingShortcuts(shortcuts)
  }, [shortcuts])

  useEffect(() => {
    if (!recordingKey) {
      setError(null)
      setLiveShortcut(null)
      return
    }

    setError(null)
    setLiveShortcut(null)
    const handleKeyDown = (e: KeyboardEvent) => {
      e.preventDefault()
      e.stopPropagation()
      setError(null)

      // Early exit for modifier-only events - but update preview!
      if (isModifierKey(e.key)) {
        setLiveShortcut(formatModifierCombination(e, isMac))
        return
      }

      // Allow Escape to cancel recording
      if (e.key === 'Escape') {
        setRecordingKey(null)
        setLiveShortcut(null)
        return
      }

      // Block system/function keys
      if (isKeyBlocked(e.key)) {
        setError(t('settings.shortcutInvalid'))
        return
      }

      const shortcut = formatShortcut(e, isMac)
      if (!shortcut) return

      setPendingShortcuts((prev) => ({ ...prev, [recordingKey]: shortcut }))
      setRecordingKey(null)
      setIsSaved(false)
      setLiveShortcut(null)
    }

    const handleKeyUp = (e: KeyboardEvent) => {
      if (isModifierKey(e.key)) {
        const combo = formatModifierCombination(e, isMac)
        setLiveShortcut(combo || null)
      }
    }

    const handleClickOutside = () => {
      setRecordingKey(null)
      setLiveShortcut(null)
    }

    window.addEventListener('keydown', handleKeyDown, { capture: true })
    window.addEventListener('keyup', handleKeyUp, { capture: true })
    window.addEventListener('click', handleClickOutside, { capture: true })
    return () => {
      window.removeEventListener('keydown', handleKeyDown, { capture: true })
      window.removeEventListener('keyup', handleKeyUp, { capture: true })
      window.removeEventListener('click', handleClickOutside, {
        capture: true,
      })
    }
  }, [recordingKey, pendingShortcuts, t, isMac])

  const [resetConfirmOpen, setResetConfirmOpen] = useState(false)

  const saveTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    return () => {
      if (saveTimeoutRef.current) {
        clearTimeout(saveTimeoutRef.current)
      }
    }
  }, [])

  const handleSave = () => {
    setShortcuts(pendingShortcuts)
    setIsSaved(true)

    if (saveTimeoutRef.current) {
      clearTimeout(saveTimeoutRef.current)
    }

    saveTimeoutRef.current = setTimeout(() => {
      setIsSaved(false)
      saveTimeoutRef.current = null
    }, 2000)
  }

  const handleReset = () => {
    setResetConfirmOpen(true)
  }

  const handleConfirmReset = () => {
    resetShortcutsStore()
    setResetConfirmOpen(false)
  }

  const renderShortcutKeys = (shortcutStr: string, kbdClass?: string) => {
    const parts = shortcutStr.split('+')

    return parts.map((part, i) => (
      <Fragment key={i}>
        <Kbd className={kbdClass}>{part}</Kbd>
        {i < parts.length - 1 && <span className='text-muted-foreground'>+</span>}
      </Fragment>
    ))
  }

  return (
    <div className='flex h-full flex-col gap-6'>
      <div className='grow space-y-6 overflow-y-auto pr-2'>
        <Section title={t('settings.keybinds')} description={t('settings.keybindsDescription')}>
          <div className='divide-y divide-border overflow-hidden rounded-xl border border-border bg-card'>
            {SHORTCUT_ITEMS.map((item) => {
              const currentVal = pendingShortcuts[item.key]
              const hasConflict = currentVal && (conflictCounts.get(currentVal) || 0) > 1
              const conflictingItem = hasConflict
                ? SHORTCUT_ITEMS.find(
                    (s) => s.key !== item.key && pendingShortcuts[s.key] === currentVal,
                  )
                : null

              return (
                <div key={item.key} className='flex items-center justify-between px-4 py-2'>
                  <div className='flex items-center gap-2'>
                    <span className='text-sm'>{t(item.labelKey)}</span>
                    {hasConflict && (
                      <div
                        title={`${t('settings.shortcutConflict')}${
                          conflictingItem ? `: ${t(conflictingItem.labelKey)}` : ''
                        }`}
                      >
                        <AlertTriangleIcon className='size-3.5 text-amber-500' />
                      </div>
                    )}
                  </div>
                  <Button
                    variant={recordingKey === item.key ? 'secondary' : 'ghost'}
                    size='sm'
                    onClick={(e) => {
                      e.stopPropagation()
                      setRecordingKey(item.key)
                    }}
                    className='group h-8 w-fit px-2 font-mono uppercase'
                  >
                    <div className='flex items-center gap-1'>
                      {recordingKey === item.key ? (
                        error ? (
                          <span className='text-xs text-destructive'>{error}</span>
                        ) : liveShortcut ? (
                          renderShortcutKeys(liveShortcut)
                        ) : (
                          <span className='text-xs text-muted-foreground italic'>
                            {t('settings.shortcutPressKey')}
                          </span>
                        )
                      ) : currentVal ? (
                        renderShortcutKeys(currentVal, 'bg-background')
                      ) : (
                        <span className='text-xs text-muted-foreground'>NONE</span>
                      )}
                    </div>
                  </Button>
                </div>
              )
            })}
          </div>
        </Section>
      </div>

      <div className='flex items-center justify-between border-t border-border pt-4'>
        <Button
          variant='ghost'
          size='sm'
          className='gap-2 text-muted-foreground hover:text-foreground'
          onClick={handleReset}
        >
          <RotateCcwIcon className='size-4' />
          {t('settings.shortcutReset')}
        </Button>
        <div className='flex items-center gap-2'>
          <Button
            variant='default'
            size='sm'
            disabled={!isDirty || isSaved}
            onClick={handleSave}
            className='min-w-32 gap-2'
          >
            {isSaved ? (
              <>
                <CheckCircleIcon className='size-4' />
                {t('common.saved')}
              </>
            ) : (
              <>
                <SaveIcon className='size-4' />
                {t('common.save')}
              </>
            )}
          </Button>
        </div>
      </div>
      <AlertDialog open={resetConfirmOpen} onOpenChange={setResetConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogTitle>{t('settings.shortcutReset')}</AlertDialogTitle>
          <AlertDialogDescription>{t('settings.shortcutResetDescription')}</AlertDialogDescription>
          <div className='flex justify-end gap-2'>
            <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction onClick={handleConfirmReset}>
              {t('common.confirm')}
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

// ── Storage ───────────────────────────────────────────────────────

function StoragePane({
  dataPath,
  httpConnectTimeout,
  httpReadTimeout,
  httpMaxRetries,
  error,
  saving,
  unchanged,
  onPathChange,
  onHttpConnectTimeoutChange,
  onHttpReadTimeoutChange,
  onHttpMaxRetriesChange,
  onApply,
}: {
  dataPath: string
  httpConnectTimeout: string
  httpReadTimeout: string
  httpMaxRetries: string
  error: string | null
  saving: boolean
  unchanged: boolean
  onPathChange: (v: string) => void
  onHttpConnectTimeoutChange: (v: string) => void
  onHttpReadTimeoutChange: (v: string) => void
  onHttpMaxRetriesChange: (v: string) => void
  onApply: () => void
}) {
  const { t } = useTranslation()
  const Badge = ({ children }: { children: ReactNode }) => (
    <span className='ml-1.5 rounded-sm bg-amber-100 px-1 py-0.5 text-[10px] font-medium text-amber-800 dark:bg-amber-900/40 dark:text-amber-200'>
      {children}
    </span>
  )
  const [confirmOpen, setConfirmOpen] = useState(false)

  return (
    <>
      <Section title={t('settings.runtime')} description={t('settings.runtimeDescription')}>
        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('settings.dataPath')}<Badge>{t('settings.restartBadge')}</Badge></Label>
          <Input type='text' value={dataPath} onChange={(e) => onPathChange(e.target.value)} />
          <p className='text-xs leading-relaxed text-muted-foreground'>
            {t('settings.dataPathDescription')}
          </p>
        </div>

        <div className='grid gap-4 md:grid-cols-2'>
          <div className='space-y-1.5'>
            <Label className='text-xs'>{t('settings.httpConnectTimeout')}<Badge>{t('settings.restartBadge')}</Badge></Label>
            <Input
              type='number'
              min='1'
              step='1'
              inputMode='numeric'
              value={httpConnectTimeout}
              onChange={(e) => onHttpConnectTimeoutChange(e.target.value)}
            />
            <p className='text-xs leading-relaxed text-muted-foreground'>
              {t('settings.httpConnectTimeoutDescription')}
            </p>
          </div>

          <div className='space-y-1.5'>
            <Label className='text-xs'>{t('settings.httpReadTimeout')}<Badge>{t('settings.restartBadge')}</Badge></Label>
            <Input
              type='number'
              min='1'
              step='1'
              inputMode='numeric'
              value={httpReadTimeout}
              onChange={(e) => onHttpReadTimeoutChange(e.target.value)}
            />
            <p className='text-xs leading-relaxed text-muted-foreground'>
              {t('settings.httpReadTimeoutDescription')}
            </p>
          </div>
        </div>

        <div className='space-y-1.5'>
          <Label className='text-xs'>{t('settings.httpMaxRetries')}<Badge>{t('settings.restartBadge')}</Badge></Label>
          <Input
            type='number'
            min='0'
            step='1'
            inputMode='numeric'
            value={httpMaxRetries}
            onChange={(e) => onHttpMaxRetriesChange(e.target.value)}
          />
          <p className='text-xs leading-relaxed text-muted-foreground'>
            {t('settings.httpMaxRetriesDescription')}
          </p>
        </div>

        {error && <p className='text-xs text-destructive'>{error}</p>}
        <div className='flex justify-end pt-1'>
          <Button
            onClick={() => setConfirmOpen(true)}
            disabled={!dataPath.trim() || saving || unchanged}
          >
            {saving ? t('settings.restartApplying') : t('settings.restartApply')}
          </Button>
        </div>
      </Section>

      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogTitle>{t('settings.restartApply')}</AlertDialogTitle>
          <AlertDialogDescription>
            {t('settings.restartRequiredDescription')}
          </AlertDialogDescription>
          <div className='flex justify-end gap-2'>
            <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                setConfirmOpen(false)
                onApply()
              }}
            >
              {t('settings.restartApply')}
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

// ── About ─────────────────────────────────────────────────────────

function AboutPane({
  version,
  latestVersion,
  status,
  isInstallingUpdate,
  onInstallUpdate,
}: {
  version?: string
  latestVersion?: string
  status: UpdaterStatus
  isInstallingUpdate: boolean
  onInstallUpdate: () => void
}) {
  const { t } = useTranslation()

  return (
    <div className='flex h-full flex-col items-center justify-center gap-5 py-8'>
      <img src='/icon-large.png' alt='Koharu' className='size-20' draggable={false} />
      <div className='text-center'>
        <h2 className='text-lg font-bold tracking-wide text-foreground'>Koharu</h2>
        <p className='mt-1 text-sm text-muted-foreground'>{t('settings.aboutTagline')}</p>
      </div>

      <div className='w-full max-w-sm rounded-xl border border-border bg-card p-4'>
        <div className='space-y-3 text-sm'>
          <InfoRow label={t('settings.aboutVersion')}>
            <div className='flex flex-col items-end gap-0.5'>
              <span className='font-mono text-xs font-medium'>{version || '...'}</span>
              {status === 'loading' && (
                <LoaderIcon className='size-3.5 animate-spin text-muted-foreground' />
              )}
              {status === 'latest' && (
                <span className='flex items-center gap-1 text-xs text-green-500'>
                  <CheckCircleIcon className='size-3.5' />
                  {t('settings.aboutLatest')}
                </span>
              )}
              {status === 'outdated' && (
                <Button
                  variant='link'
                  size='xs'
                  onClick={onInstallUpdate}
                  disabled={isInstallingUpdate}
                  className='h-auto gap-1 p-0 text-amber-500'
                >
                  {isInstallingUpdate ? (
                    <LoaderIcon className='size-3.5 animate-spin' />
                  ) : (
                    <AlertCircleIcon className='size-3.5' />
                  )}
                  {t('settings.aboutUpdate', { version: latestVersion })}
                </Button>
              )}
            </div>
          </InfoRow>
          <InfoRow label={t('settings.aboutAuthor')}>
            <Button
              variant='link'
              size='xs'
              onClick={() => void openExternalUrl('https://github.com/mayocream')}
            >
              Mayo
            </Button>
          </InfoRow>
          <InfoRow label={t('settings.aboutRepository')}>
            <Button
              variant='link'
              size='xs'
              onClick={() => void openExternalUrl(`https://github.com/${GITHUB_REPO}`)}
            >
              GitHub
            </Button>
          </InfoRow>
        </div>
      </div>
    </div>
  )
}

// ── Shared ────────────────────────────────────────────────────────

function Section({
  title,
  description,
  children,
}: {
  title: string
  description?: string
  children: ReactNode
}) {
  return (
    <div className='space-y-3'>
      <div>
        <h3 className='text-sm font-semibold text-foreground'>{title}</h3>
        {description && (
          <p className='mt-0.5 text-xs leading-relaxed text-muted-foreground'>{description}</p>
        )}
      </div>
      {children}
    </div>
  )
}

function InfoRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className='flex items-center justify-between'>
      <span className='text-muted-foreground'>{label}</span>
      <div className='flex items-center'>{children}</div>
    </div>
  )
}
