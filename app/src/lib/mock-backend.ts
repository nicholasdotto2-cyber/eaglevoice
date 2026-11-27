/**
 * Backend finto per vedere e fotografare l'interfaccia nel browser, senza Rust.
 * Si attiva solo con `VITE_MOCK_BACKEND=1` (vedi `mock-install.ts`). Risponde ai
 * comandi di Handy e a quelli aggiunti da eagleVoice con dati inventati, e
 * gestisce gli eventi in memoria. Da console: `window.__flowMock.emit(evento, payload)`.
 *
 * I dati sono di esempio, inventati, non dettature reali.
 */
import { mockConvertFileSrc, mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type {
  CleanupSettings,
  DictionaryEntry,
  EngineState,
  LearnedCandidate,
  LearnedWord,
  TransformSlot,
  TransformsState,
} from "./backend";
import { DEFAULT_ASR_SETTINGS, DEFAULT_CLEANUP_SETTINGS } from "./backend";
import type { FlowHistoryEntry } from "./history";

const nowSec = () => Math.floor(Date.now() / 1000);

const binding = (id: string, name: string, current: string) => ({
  id,
  name,
  description: name,
  default_binding: current,
  current_binding: current,
});

const buildSettings = () => ({
  settings_schema_version: 3,
  bindings: {
    transcribe: binding("transcribe", "Transcribe", "ctrl_left+meta_left"),
    cancel: binding("cancel", "Cancel", "escape"),
    polish: binding("polish", "Polish", "super+alt+1"),
    transform_2: binding("transform_2", "Transform 2", "super+alt+2"),
    transform_3: binding("transform_3", "Transform 3", "super+alt+3"),
    ...Object.fromEntries(
      [4, 5, 6, 7, 8, 9].map((n) => [`transform_${n}`, binding(`transform_${n}`, `Transform ${n}`, "")]),
    ),
  },
  shortcut_activation: "hold_or_toggle",
  hold_threshold_ms: 200,
  audio_feedback: true,
  audio_feedback_volume: 0.6,
  sound_theme: "marimba",
  start_hidden: false,
  autostart_enabled: true,
  show_whats_new_on_update: false,
  whats_new_last_seen_version: "0.9.8",
  selected_model: "whisper-large-v3-turbo",
  onboarding_completed: true,
  always_on_microphone: false,
  selected_microphone: null,
  selected_channel: null,
  clamshell_microphone: null,
  selected_output_device: null,
  translate_to_english: false,
  selected_language: "it",
  // Gli screenshot scelgono il bordo con localStorage "eagle.mock.edge".
  overlay_position: localStorage.getItem("eagle.mock.edge") ?? "bottom",
  overlay_dock_offset: 0.5,
  overlay_style: "minimal",
  debug_mode: false,
  log_level: "info",
  custom_words: [],
  model_unload_timeout: "never",
  word_correction_threshold: 0.18,
  history_limit: 500,
  recording_retention_period: "preserve_limit",
  paste_method: "ctrl_v",
  clipboard_handling: "dont_modify",
  auto_submit: false,
  auto_submit_key: "enter",
  post_process_enabled: false,
  post_process_provider_id: "openai",
  post_process_providers: [],
  post_process_api_keys: {},
  post_process_models: {},
  post_process_prompts: [],
  post_process_selected_prompt_id: null,
  mute_while_recording: false,
  append_trailing_space: true,
  app_language: "it",
  // Il tema scelto per gli screenshot arriva da localStorage "handy.theme".
  theme: localStorage.getItem("handy.theme") ?? "light",
  experimental_enabled: false,
  lazy_stream_close: false,
  keyboard_implementation: "tauri",
  show_tray_icon: true,
  paste_delay_ms: 60,
  paste_delay_after_ms: 60,
  reliable_paste: true,
  typing_tool: "auto",
  external_script_path: null,
  filler_word_removal_enabled: true,
  custom_filler_words: null,
  chinese_script: "as_transcribed",
  transcribe_accelerator: "auto",
  transcribe_gpu_device: null,
  extra_recording_buffer_ms: 0,
  vad_enabled: true,
  vad_backend: "silero",
  long_form_chunking: true,
  flow_bar_always_visible: true,
  // Gli screenshot accendono "p/m all'incolla" con localStorage "eagle.mock.pasteWpm".
  flow_bar_paste_wpm: localStorage.getItem("eagle.mock.pasteWpm") === "1",
  asr: {
    ...DEFAULT_ASR_SETTINGS,
    ranked_microphones: ["Cuffie USB", "Microfono (Realtek Audio)", "Microfono da tavolo"],
    whisper_bin_dir: "C:\\eaglevoice\\bin\\whisper",
    whisper_model_path:
      "C:\\eaglevoice\\models\\ggml-large-v3-turbo-q8_0.bin",
  },
  cleanup: {
    ...DEFAULT_CLEANUP_SETTINGS,
    app_formats: [{ app_match: "WhatsApp", format: "chat" }],
    llama_bin_dir: "C:\\eaglevoice\\bin\\llama",
    llama_model_path:
      "C:\\eaglevoice\\models\\Qwen3.5-4B-Q4_K_M.gguf",
  } satisfies CleanupSettings,
});

const ready = (model: string, vram: number): EngineState => ({
  state: "ready",
  detail: null,
  model,
  vram_mb: vram,
});

const daysAgo = (d: number) =>
  new Date(Date.now() - d * 86400000).toISOString();

const DICTIONARY: DictionaryEntry[] = [
  { id: "d1", phrase: "GitHub", replacement: null, is_snippet: false, starred: true, frequency: 48, last_used: daysAgo(0), source: "manual" },
  { id: "d2", phrase: "Nomeprova", replacement: null, is_snippet: false, starred: false, frequency: 17, last_used: daysAgo(1), source: "imported" },
  { id: "d3", phrase: "DemoLab", replacement: null, is_snippet: false, starred: false, frequency: 9, last_used: daysAgo(3), source: "imported" },
  { id: "d4", phrase: "data base", replacement: "database", is_snippet: false, starred: false, frequency: 22, last_used: daysAgo(0), source: "learned" },
  { id: "d5", phrase: "nome esempio", replacement: "Nome Esempio", is_snippet: false, starred: true, frequency: 12, last_used: daysAgo(2), source: "manual" },
  { id: "d6", phrase: "la mia firma", replacement: "Nome Esempio\nDitta di prova\nesempio.example", is_snippet: true, starred: false, frequency: 4, last_used: daysAgo(6), source: "manual" },
  { id: "d7", phrase: "Python", replacement: null, is_snippet: false, starred: false, frequency: 0, last_used: null, source: "manual" },
];

const CANDIDATES: LearnedCandidate[] = [
  { id: "c1", wrong: "Nomeprov", right: "Nomeprova", seen_count: 3, example: "parola uno Nomeprova parola due", first_seen: daysAgo(4) },
  { id: "c2", wrong: "Pitone", right: "Python", seen_count: 2, example: "parola uno Python parola due", first_seen: daysAgo(2) },
];

const HISTORY: FlowHistoryEntry[] = [
  {
    id: 3, file_name: "rec-3.wav", timestamp: nowSec() - 180, saved: false, title: "",
    transcription_text: "parola uno ehm parola due anzi no parola tre numero dieci",
    post_processed_text: null, post_process_prompt: null, post_process_requested: false,
    raw_text: "parola uno ehm parola due anzi no parola tre numero dieci",
    cleaned_text: "Parola uno parola tre numero 10.",
    edited_text: null, app_name: "WhatsApp", category: "chat_personal", latency_ms: 840, used_quality: false, language: "it",
  },
  {
    id: 2, file_name: "rec-2.wav", timestamp: nowSec() - 3600, saved: true, title: "",
    transcription_text: "parola uno tre file di prova parola due con nomeprov",
    post_processed_text: null, post_process_prompt: null, post_process_requested: false,
    raw_text: "parola uno tre file di prova parola due con nomeprov",
    cleaned_text: "Parola uno tre file di prova, parola due con Nomeprov.",
    edited_text: "Parola uno tre file di prova, parola due con Nomeprova.",
    app_name: "Claude", category: "ai", latency_ms: 1120, used_quality: false, language: "it",
  },
  {
    id: 1, file_name: "rec-1.wav", timestamp: nowSec() - 86400, saved: false, title: "",
    transcription_text: "parola uno nome esempio parola due nomeprova parola tre",
    post_processed_text: null, post_process_prompt: null, post_process_requested: false,
    raw_text: "parola uno nome esempio parola due nomeprova parola tre",
    cleaned_text: "Parola uno Nome Esempio, parola due Nomeprova parola tre.",
    edited_text: null, app_name: "Outlook", category: "email", latency_ms: 2310, used_quality: true, archived: true,
  },
];

const LEARNED: LearnedWord[] = [
  { id: "l1", wrong: "tavlo", right: "tavolo", count: 1, created_at: daysAgo(0), active: true },
  { id: "l2", wrong: "Nomeprov", right: "Nomeprova", count: 3, created_at: daysAgo(1), active: true },
  { id: "l3", wrong: "github", right: "GitHub", count: 2, created_at: daysAgo(5), active: false },
];

const TRANSFORMS: TransformsState = {
  enabled: true,
  auto_apply: { active: false, slot: 1 },
  max_words: 3000,
  local_max_words: 1800,
  slots: [
    { slot: 1, name: "Polish", prompt: "", shortcut: "super+alt+1", examples: [], builtin: true },
    { slot: 2, name: "Prompt Builder", prompt: "", shortcut: "super+alt+2", examples: [], builtin: true },
    {
      slot: 3,
      name: "Email cordiale",
      prompt: "Riscrivi come email cordiale, con saluto e firma.",
      shortcut: "super+alt+3",
      examples: [],
      builtin: false,
    },
  ],
};

export const installMockBackend = (): void => {
  const settings: Record<string, unknown> = buildSettings();
  let dictionary = [...DICTIONARY];
  let candidates = [...CANDIDATES];
  let history = [...HISTORY];
  let learned = [...LEARNED];
  let transforms: TransformsState = structuredClone(TRANSFORMS);
  let nextId = 100;
  /** Chiamate a flow_expand_bar e flow_rest_bar, lette dagli script degli screenshot. */
  const barCalls: string[] = [];

  const w = window as unknown as Record<string, unknown>;
  w.__TAURI_OS_PLUGIN_INTERNALS__ = {
    platform: "windows",
    os_type: "windows",
    family: "windows",
    version: "10.0.22631",
    arch: "x86_64",
    exe_extension: "exe",
    eol: "\r\n",
  };
  mockWindows(window.location.pathname.includes("overlay") ? "recording_overlay" : "main");
  mockConvertFileSrc("windows");

  mockIPC(
    (cmd, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      switch (cmd) {
        // Handy
        case "get_app_settings":
        case "get_default_settings":
          return settings;
        case "get_history_entries": {
          const limit = (args.limit as number | null) ?? 30;
          return { entries: history.slice(0, limit), has_more: false };
        }
        case "delete_history_entry":
          history = history.filter((h) => h.id !== args.id);
          return null;
        case "archive_history_entry": {
          history = history.map((h) =>
            h.id === args.id ? { ...h, archived: args.archived as boolean } : h,
          );
          const entry = history.find((h) => h.id === args.id);
          if (!entry) throw `voce ${String(args.id)} non trovata`;
          setTimeout(() => emit("history-update-payload", { action: "updated", entry }), 0);
          return null;
        }
        case "toggle_history_entry_saved":
          history = history.map((h) => (h.id === args.id ? { ...h, saved: !h.saved } : h));
          return null;
        case "get_audio_file_path":
          return `C:\\mock\\${String(args.fileName)}`;
        case "get_available_microphones":
          return [
            { index: "0", name: "Microfono (Realtek Audio)", is_default: true },
            { index: "1", name: "Cuffie USB", is_default: false },
          ];
        case "get_available_output_devices":
          return [{ index: "0", name: "Altoparlanti (Realtek Audio)", is_default: true }];
        case "get_selected_microphone":
          return "Microfono (Realtek Audio)";
        case "get_selected_output_device":
          return "Altoparlanti (Realtek Audio)";
        case "get_microphone_channels":
          return [];
        case "check_custom_sounds":
          return { start: false, stop: false };
        case "is_portable":
        case "is_recording":
        case "is_model_loading":
        case "is_laptop":
          return false;
        case "get_windows_microphone_permission_status":
          return { supported: false, overall_access: "allowed" };
        case "get_retired_models":
        case "get_available_models":
          return [];
        case "get_current_model":
          return "";
        case "get_secure_input_status":
          return { active: false };
        case "plugin:os|locale":
          return "it-IT";
        case "plugin:app|version":
          return "0.9.8";
        case "plugin:autostart|is_enabled":
          return true;
        // Comandi di eagleVoice
        case "flow_stop":
        case "flow_paste_last":
        case "cancel_operation":
        case "asr_restart_engine":
        case "cleanup_restart_engine":
          return null;
        case "update_settings_section":
          settings[args.section as string] = args.value;
          return null;
        case "set_flow_bar_visible":
          settings.flow_bar_always_visible = args.visible;
          return null;
        // Come flow.rs: misura piena per una notifica, poi di nuovo a riposo.
        case "flow_expand_bar":
          barCalls.push(cmd);
          setTimeout(() => emit("show-overlay", "notice"), 0);
          return null;
        case "flow_rest_bar":
          barCalls.push(cmd);
          setTimeout(() => {
            if (settings.flow_bar_always_visible) {
              emit("flow:state", { state: "idle", detail: null, since_ms: Date.now() });
              emit("show-overlay", "idle");
            } else {
              emit("hide-overlay", null);
            }
          }, 0);
          return null;
        // Bolla di Eagle Voice: il trascinamento e il clic veri li fa overlay.rs.
        case "flow_bar_press":
          barCalls.push(cmd);
          return null;
        case "flow_bar_dock":
          return {
            edge: settings.overlay_position,
            offset: (settings.overlay_dock_offset as number | undefined) ?? 0.5,
          };
        case "plugin:clipboard-manager|write_text":
          return null;
        case "asr_get_state":
          return ready("large-v3-turbo Q8_0", 1650);
        case "cleanup_get_state":
          return ready("Qwen3.5-4B Q4_K_M", 3100);
        case "cleanup_quality_key_present":
          return true;
        case "cleanup_preview":
          return "Parola uno parola tre numero 10.";
        case "dictionary_list":
          return dictionary;
        case "dictionary_candidates":
          return candidates;
        case "dictionary_add": {
          const entry = args.entry as Partial<DictionaryEntry>;
          const created: DictionaryEntry = {
            id: `d${nextId++}`,
            phrase: entry.phrase ?? "",
            replacement: entry.replacement ?? null,
            is_snippet: entry.is_snippet ?? false,
            starred: entry.starred ?? false,
            frequency: 0,
            last_used: null,
            source: "manual",
          };
          dictionary = [...dictionary, created];
          return created;
        }
        case "dictionary_update": {
          const entry = args.entry as DictionaryEntry;
          dictionary = dictionary.map((d) => (d.id === entry.id ? entry : d));
          return null;
        }
        case "dictionary_delete":
          dictionary = dictionary.filter((d) => d.id !== args.id);
          return null;
        case "dictionary_accept_candidate":
        case "dictionary_reject_candidate": {
          const c = candidates.find((x) => x.id === args.id);
          candidates = candidates.filter((x) => x.id !== args.id);
          if (c && cmd === "dictionary_accept_candidate") {
            dictionary = [
              ...dictionary,
              { id: `d${nextId++}`, phrase: c.wrong, replacement: c.right, is_snippet: false, starred: false, frequency: c.seen_count, last_used: null, source: "learned" },
            ];
          }
          return null;
        }
        case "dictionary_import_from_json":
          return 36;
        // Parole imparate e Transforms.
        case "list_learned_words":
          return learned;
        // Come il backend (e165ea2): Annulla lascia la voce con active falso.
        case "undo_learned_word":
          learned = learned.map((l) => (l.id === args.id ? { ...l, active: false } : l));
          return null;
        case "delete_learned_word":
          learned = learned.filter((l) => l.id !== args.id);
          return null;
        case "get_transforms":
          return transforms;
        // save, delete, reset e opzioni non restituiscono lo stato: l'hub rilegge.
        case "save_transform": {
          const slot = args.slot as TransformSlot;
          const exists = transforms.slots.some((s) => s.slot === slot.slot);
          transforms = {
            ...transforms,
            slots: exists
              ? transforms.slots.map((s) => (s.slot === slot.slot ? slot : s))
              : [...transforms.slots, slot].sort((a, b) => a.slot - b.slot),
          };
          return null;
        }
        case "delete_transform":
          transforms = {
            ...transforms,
            slots: transforms.slots.filter((s) => s.slot !== args.slot || s.builtin),
          };
          return null;
        case "reset_transforms":
          transforms = structuredClone(TRANSFORMS);
          return null;
        case "set_transforms_options":
          transforms = {
            ...transforms,
            enabled: args.enabled as boolean,
            auto_apply: { active: args.autoApplyActive as boolean, slot: args.autoApplySlot as number },
          };
          return null;
        case "change_binding": {
          const bindings = settings.bindings as Record<string, { current_binding: string }>;
          const b = bindings[args.id as string];
          if (b) b.current_binding = args.binding as string;
          return { success: true, binding: b ?? null, error: null };
        }
        case "undo_transform":
        case "retry_transform":
          return null;
        case "set_flow_bar_paste_wpm":
          settings.flow_bar_paste_wpm = args.enabled;
          return null;
        default:
          if (cmd.startsWith("get_available")) return [];
          return null;
      }
    },
    { shouldMockEvents: true },
  );

  w.__flowMock = {
    emit: (event: string, payload: unknown) => emit(event, payload),
    settings,
    barCalls,
  };
  console.info("[mock-backend] attivo");
};
