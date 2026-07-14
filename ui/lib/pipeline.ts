/**
 * Shared pipeline options factory.
 *
 * Every pipeline caller needs the same set of runtime options (target language,
 * system prompt, default font, reading order).  This helper reads them from
 * their Zustand stores so callers don't repeat the same 4 lines.
 *
 * `pages` is **not** included here because each caller scopes pages
 * differently (single-page toolbar vs whole-project MenuBar).
 */
import { useEditorUiStore } from '@/lib/stores/editorUiStore'
import { usePreferencesStore } from '@/lib/stores/preferencesStore'

export function pipelineOptions() {
  const editor = useEditorUiStore.getState()
  const prefs = usePreferencesStore.getState()
  return {
    targetLanguage: editor.selectedLanguage,
    systemPrompt: prefs.customSystemPrompt,
    defaultFont: prefs.defaultFont,
    readingOrder: editor.readingOrder === 'custom' ? undefined : editor.readingOrder,
  }
}
