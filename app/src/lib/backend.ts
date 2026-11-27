/**
 * Adapter tipizzato verso i comandi Tauri aggiunti da eagleVoice.
 *
 * Ogni comando che `bindings.ts` non genera ancora passa da qui. Quando il
 * comando Rust non esiste, Tauri risponde con un errore del tipo
 * "Command flow_stop not found": la funzione lo traduce in
 * `{ ok: false, notConnected: true }` e l'interfaccia mostra "non collegato"
 * invece di rompersi. Quando il comando è registrato e `bindings.ts` è
 * rigenerato, l'import si sostituisce con `commands.*`.
 */
import { invoke } from "@tauri-apps/api/core";
import { commands, type TransformSlot } from "@/bindings";

// ---------- Tipi dei comandi e degli eventi ----------

export type FlowStateName =
  | "idle"
  | "listening"
  | "transcribing"
  | "cleaning"
  | "pasting"
  | "pasted"
  | "cancelled"
  | "error";

export interface FlowStateEvent {
  state: FlowStateName;
  detail: string | null;
  since_ms: number;
}

export interface PartialTextEvent {
  text: string;
}

export type EngineStateName = "stopped" | "starting" | "ready" | "error";

/** Payload di `asr:engine-state` e `cleanup:engine-state`. */
export interface EngineState {
  state: EngineStateName;
  detail: string | null;
  model: string | null;
  vram_mb: number | null;
}

/** Ipotesi: `asr_get_state()` e `cleanup_get_state()` restituiscono la stessa forma dell'evento. */
export type AsrState = EngineState;
export type CleanupState = EngineState;

export type CleanupLevel = "exact" | "tidy" | "smooth" | "rewrite";
export type TranscriptionFormat = "smart" | "raw" | "chat" | "lower";
export type Tone =
  | "default"
  | "polished"
  | "relaxed"
  | "loose"
  | "lively";
export type StyleCategory = "chat" | "team" | "mail" | "general";

export interface AppFormatRule {
  app_match: string;
  format: TranscriptionFormat;
}

export interface PolishInstruction {
  id: string;
  label: string;
  enabled: boolean;
}

export interface CleanupSettings {
  level: CleanupLevel;
  default_format: TranscriptionFormat;
  app_formats: AppFormatRule[];
  styles: Record<StyleCategory, Tone>;
  backtrack_enabled: boolean;
  press_enter_enabled: boolean;
  lists_enabled: boolean;
  quality_mode: boolean;
  quality_model: string;
  polish_instructions: PolishInstruction[];
  command_mode_enabled: boolean;
  llama_bin_dir: string;
  llama_model_path: string;
  skip_cleanup_under_words: number;
}

export interface AsrSettings {
  engine: "whisper_external" | "handy_inprocess";
  whisper_model_path: string;
  whisper_bin_dir: string;
  languages: string[];
  primary_language: string;
  /** Rileva la lingua a ogni dettatura fra `languages`; spento usa `primary_language`. */
  language_detection: boolean;
  chunked_decode: boolean;
  max_recording_minutes: number;
  prompt_max_tokens: number;
  /** Nomi dei microfoni in ordine di preferenza. Se manca vale lista vuota. */
  ranked_microphones?: string[];
}

export type AppCategory =
  | "email"
  | "chat_personal"
  | "chat_work"
  | "ai"
  | "terminal"
  | "document"
  | "code"
  | "browser"
  | "other";

export interface CleanupContext {
  app_name: string | null;
  category: AppCategory | null;
  url: string | null;
  field_before: string | null;
  field_selected: string | null;
  field_after: string | null;
  format: TranscriptionFormat | null;
  level: CleanupLevel | null;
  style: Tone | null;
  language: string | null;
  proper_nouns: string[];
}

export interface DictionaryEntry {
  id: string;
  phrase: string;
  replacement: string | null;
  is_snippet: boolean;
  starred: boolean;
  frequency: number;
  last_used: string | null;
  source: "manual" | "learned" | "imported";
}

/** Voce nuova: l'id, la frequenza e l'ultimo uso li assegna il backend. */
export type NewDictionaryEntry = Pick<
  DictionaryEntry,
  "phrase" | "replacement" | "is_snippet" | "starred"
>;

export interface LearnedCandidate {
  id: string;
  wrong: string;
  right: string;
  seen_count: number;
  example: string;
  first_seen: string;
}

/** Campi che eagleVoice aggiunge a `HistoryEntry` (tutti facoltativi finché mancano). */
export interface HistoryExtraFields {
  raw_text?: string | null;
  cleaned_text?: string | null;
  edited_text?: string | null;
  app_name?: string | null;
  category?: string | null;
  latency_ms?: number | null;
  used_quality?: boolean | null;
}

export type SettingsSection = "asr" | "cleanup" | "win";

// ---------- Risultato ----------

export type BackendResult<T> =
  | { ok: true; data: T }
  | { ok: false; notConnected: boolean; error: string };

const NOT_FOUND_RE = /not found|unknown command|not allowed|isn't registered/i;

/** True se l'errore dice che il comando Rust non esiste ancora. */
export const isNotConnectedError = (error: unknown): boolean => {
  const text = typeof error === "string" ? error : String(error);
  return NOT_FOUND_RE.test(text) || text.includes("__TAURI_INTERNALS__");
};

const call = async <T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<BackendResult<T>> => {
  try {
    const data = await invoke<T>(cmd, args);
    return { ok: true, data };
  } catch (error) {
    return {
      ok: false,
      notConnected: isNotConnectedError(error),
      error: typeof error === "string" ? error : String(error),
    };
  }
};

// ---------- Barra di dettatura (flow.rs) ----------

export const flowStop = () => call<null>("flow_stop");
export const flowPasteLast = () => call<null>("flow_paste_last");
/** Salva `flow_bar_always_visible` e mostra o nasconde subito la barra a riposo. */
export const setFlowBarVisible = (visible: boolean) =>
  call<null>("set_flow_bar_visible", { visible });
/** Finestra della bolla a 360x190 per mostrare un avviso (anche durante la dettatura). */
export const flowExpandBar = () => call<null>("flow_expand_bar");
/** Avviso chiuso: la finestra torna alla misura dello stato (52x52 a riposo) o si nasconde se `flow_bar_always_visible` è false. */
export const flowRestBar = () => call<null>("flow_rest_bar");

// ---------- Impostazioni (flow.rs) ----------

export const updateSettingsSection = <T>(section: SettingsSection, value: T) =>
  call<null>("update_settings_section", { section, value });

// ---------- Modulo asr (asr\commands.rs) ----------

export const asrGetState = () => call<AsrState>("asr_get_state");
export const asrRestartEngine = () => call<null>("asr_restart_engine");
export const asrTranscribeFile = (path: string, vocabulary: string[]) =>
  call<string>("asr_transcribe_file", { path, vocabulary });

// ---------- Modulo cleanup (cleanup\commands.rs) ----------

export const cleanupGetState = () => call<CleanupState>("cleanup_get_state");
export const cleanupRestartEngine = () => call<null>("cleanup_restart_engine");
/** Legge solo la presenza di ANTHROPIC_API_KEY, mai il valore. */
export const cleanupQualityKeyPresent = () =>
  call<boolean>("cleanup_quality_key_present");
export const cleanupPreview = (raw: string, context: CleanupContext) =>
  call<string>("cleanup_preview", { raw, context });
export const cleanupPolish = (text: string, instructions: string[]) =>
  call<string>("cleanup_polish", { text, instructions });

// ---------- Dizionario (cleanup\commands.rs) ----------

export const dictionaryList = () => call<DictionaryEntry[]>("dictionary_list");
/** Rust riceve la voce completa e assegna l'id quando arriva vuoto. */
export const dictionaryAdd = (entry: NewDictionaryEntry) =>
  call<DictionaryEntry>("dictionary_add", {
    entry: { id: "", frequency: 0, last_used: null, source: "manual", ...entry },
  });
export const dictionaryUpdate = (entry: DictionaryEntry) =>
  call<null>("dictionary_update", { entry });
export const dictionaryDelete = (id: string) =>
  call<null>("dictionary_delete", { id });
export const dictionaryCandidates = () =>
  call<LearnedCandidate[]>("dictionary_candidates");
export const dictionaryAcceptCandidate = (id: string) =>
  call<null>("dictionary_accept_candidate", { id });
export const dictionaryRejectCandidate = (id: string) =>
  call<null>("dictionary_reject_candidate", { id });
export const dictionaryImportFromJson = (path: string) =>
  call<number>("dictionary_import_from_json", { path });

// ---------- Dizionario che impara e Transforms: comandi di bindings.ts ----------

export type {
  AutoApply,
  LearnedWord,
  TransformSlot,
  TransformsState,
} from "@/bindings";

/** Payload di `cleanup:learned`: la parola è appena entrata nel dizionario. */
export interface LearnedEvent {
  id: string;
  wrong: string;
  right: string;
}

export type TransformPhase = "running" | "done" | "error";

/**
 * Payload di `flow:transform`: `running`, poi `done` o `error`. `done` con
 * `changes` 0 vuol dire nessuna modifica (anche dopo Annulla, con before e
 * after invertiti). `message` si mostra così com'è nello stato `error`.
 */
export interface FlowTransformEvent {
  state: TransformPhase;
  name: string;
  changes: number;
  before: string;
  after: string;
  message?: string | null;
}

type BindingsResult<T> = { status: "ok"; data: T } | { status: "error"; error: unknown };

/** Da un comando dei bindings che lancia in errore a `BackendResult`. */
const wrapPlain = async <T>(run: () => Promise<T>): Promise<BackendResult<T>> =>
  wrap(async () => ({ status: "ok", data: await run() }) as BindingsResult<T>);

/** Da `Result` dei bindings (o da un comando che lancia) a `BackendResult`. */
const wrap = async <T>(run: () => Promise<BindingsResult<T>>): Promise<BackendResult<T>> => {
  try {
    const r = await run();
    if (r.status === "ok") return { ok: true, data: r.data };
    const error = typeof r.error === "string" ? r.error : String(r.error);
    return { ok: false, notConnected: isNotConnectedError(error), error };
  } catch (error) {
    return {
      ok: false,
      notConnected: isNotConnectedError(error),
      error: typeof error === "string" ? error : String(error),
    };
  }
};

export const listLearnedWords = () => wrap(() => commands.listLearnedWords());
/** "Annulla": la voce resta in lista con `active` falso e la sostituzione sparisce. */
export const undoLearnedWord = (id: string) => wrap(() => commands.undoLearnedWord(id));
export const deleteLearnedWord = (id: string) => wrap(() => commands.deleteLearnedWord(id));

export const getTransforms = () => wrapPlain(() => commands.getTransforms());
export const saveTransform = (slot: TransformSlot) => wrap(() => commands.saveTransform(slot));
export const deleteTransform = (slot: number) => wrap(() => commands.deleteTransform(slot));
export const resetTransforms = () =>
  wrapPlain(() => commands.resetTransforms().then(() => null));
export const setTransformsOptions = (
  enabled: boolean,
  autoApplyActive: boolean,
  autoApplySlot: number,
) => wrap(() => commands.setTransformsOptions(enabled, autoApplyActive, autoApplySlot));
/** Annulla e Riprova del diff (flow.rs): in errore il messaggio si mostra così com'è. */
export const undoTransform = () => wrap(() => commands.undoTransform());
export const retryTransform = () => wrap(() => commands.retryTransform());

/** "p/m" accanto alla spunta all'incolla: campo `flow_bar_paste_wpm` di AppSettings. */
export const setFlowBarPasteWpm = (enabled: boolean) =>
  wrapPlain(() => commands.setFlowBarPasteWpm(enabled).then(() => null));

/** Id del binding della scorciatoia di un Transform: slot 1 `polish`, slot 2-9 `transform_N`. */
export const transformBindingId = (slot: number): string =>
  slot === 1 ? "polish" : `transform_${slot}`;

// ---------- Valori di default (specchio dei Default Rust attesi) ----------

export const DEFAULT_POLISH_INSTRUCTIONS: PolishInstruction[] = [
  { id: "trim", label: "Cut repetition and padding", enabled: true },
  { id: "plain", label: "Use plain words", enabled: true },
  { id: "order", label: "Put the key facts first", enabled: true },
  { id: "paragraphs", label: "Split into short paragraphs", enabled: true },
  { id: "voice", label: "Keep the writer's voice", enabled: false },
  { id: "ask", label: "End with the question or request", enabled: false },
];

/** Default della pulizia (specchio dei Default Rust). */
export const DEFAULT_CLEANUP_SETTINGS: CleanupSettings = {
  level: "tidy",
  default_format: "smart",
  app_formats: [],
  styles: { chat: "relaxed", team: "relaxed", mail: "relaxed", general: "relaxed" },
  backtrack_enabled: true,
  press_enter_enabled: false,
  lists_enabled: true,
  quality_mode: false,
  quality_model: "claude-haiku-5-5",
  polish_instructions: DEFAULT_POLISH_INSTRUCTIONS,
  command_mode_enabled: false,
  llama_bin_dir: "",
  llama_model_path: "",
  skip_cleanup_under_words: 0,
};

export const DEFAULT_ASR_SETTINGS: AsrSettings = {
  engine: "whisper_external",
  whisper_model_path: "",
  whisper_bin_dir: "",
  languages: ["it", "en"],
  primary_language: "it",
  language_detection: false,
  chunked_decode: true,
  max_recording_minutes: 20,
  prompt_max_tokens: 224,
};

export const EMPTY_CLEANUP_CONTEXT: CleanupContext = {
  app_name: null,
  category: null,
  url: null,
  field_before: null,
  field_selected: null,
  field_after: null,
  format: null,
  level: null,
  style: null,
  language: null,
  proper_nouns: [],
};

// ---------- Eventi ----------

export const FLOW_EVENTS = {
  state: "flow:state",
  partialText: "flow:partial-text",
  micLevel: "mic-level",
  recordingError: "recording-error",
  asrEngineState: "asr:engine-state",
  cleanupEngineState: "cleanup:engine-state",
  dictionaryChanged: "cleanup:dictionary-changed",
  /** `{ text, reason }`: l'incolla non è riuscito. */
  pasteFailed: "flow:paste-failed",
  /** `{ words, audio_secs, wpm }`: a ogni dettatura incollata. */
  dictationStats: "flow:dictation-stats",
  /** `{ seconds }`: manca un minuto al tetto di registrazione. */
  durationWarning: "asr:duration-warning",
  /** `{ edge, offset }`: la bolla si è agganciata a un bordo (dopo un trascinamento o un cambio di misura). */
  dock: "flow:dock",
  /** `{ id, wrong, right }`: una parola imparata è entrata nel dizionario. */
  learned: "cleanup:learned",
  /** `{ state, name, changes, before, after }`: un Transform è partito, finito o fallito. */
  transform: "flow:transform",
  /** Evento solo frontend: la barra chiede all'hub di aprire una sezione. */
  hubNavigate: "hub:navigate",
} as const;

export interface HubNavigateEvent {
  section: string;
}
