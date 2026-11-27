/**
 * Logica pura della barra di dettatura e delle notifiche: niente React, niente Tauri,
 * così si prova con `bun test`.
 */
import type { EngineState, FlowStateName } from "./backend";

// ---------- Parole al minuto (evento `flow:dictation-stats`) ----------

/** Payload di `flow:dictation-stats`: parole, secondi di audio e parole al minuto della dettatura incollata. */
export interface DictationStats {
  words: number;
  audio_secs: number;
  /** `null` quando l'audio dura meno di un secondo. */
  wpm: number | null;
}

/** Parole al minuto della sessione: ultima dettatura e somme per la media. */
export interface SessionWpm {
  last: DictationStats | null;
  /** Dettature che entrano nella media (quelle con `wpm` non nullo). */
  count: number;
  words: number;
  audioSecs: number;
}

export const EMPTY_SESSION_WPM: SessionWpm = {
  last: null,
  count: 0,
  words: 0,
  audioSecs: 0,
};

/** Aggiunge una dettatura alla sessione. Le dettature senza `wpm` aggiornano solo l'ultima. */
export const addDictationStats = (
  session: SessionWpm,
  stats: DictationStats,
): SessionWpm => {
  const valid =
    stats.wpm !== null &&
    Number.isFinite(stats.audio_secs) &&
    stats.audio_secs > 0 &&
    Number.isFinite(stats.words);
  if (!valid) return { ...session, last: stats };
  return {
    last: stats,
    count: session.count + 1,
    words: session.words + stats.words,
    audioSecs: session.audioSecs + stats.audio_secs,
  };
};

/** Media della sessione pesata sulla durata: parole totali diviso minuti di audio totali. */
export const sessionAverageWpm = (session: SessionWpm): number | null =>
  session.count === 0 || session.audioSecs <= 0
    ? null
    : Math.round(session.words / (session.audioSecs / 60));

// ---------- Incolla non riuscito (evento `flow:paste-failed`) ----------

export interface PasteFailedEvent {
  text: string;
  reason: string | null;
}

/** Quanti caratteri del testo mostra la notifica nella barra. */
export const PASTE_PREVIEW_CHARS = 48;

/** Inizio del testo su una riga: spazi compattati, puntini se è più lungo. */
export const pastePreview = (
  text: string,
  max: number = PASTE_PREVIEW_CHARS,
): string => {
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= max) return flat;
  return `${flat.slice(0, max).trimEnd()}...`;
};

/** Timer "m:ss" della barra. */
export const formatElapsed = (elapsedMs: number): string => {
  const total = Math.max(0, Math.floor(elapsedMs / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
};

/**
 * Scorciatoia corta per la barra (la pillola a riposo è larga al massimo 116 px):
 * "Left Ctrl + Left Meta" diventa "Ctrl + Win".
 */
export const compactShortcut = (label: string): string =>
  label
    .split("+")
    .map((part) =>
      part
        .trim()
        .replace(/^(Left|Right)\s+/i, "")
        .replace(/^(Meta|Super|Command|Cmd|Windows)$/i, "Win"),
    )
    .filter(Boolean)
    .join(" + ");

// ---------- Stati della barra ----------

export type FlowBarPhase = "listening" | "working" | "done" | "error";

export const flowPhase = (state: FlowStateName): FlowBarPhase | null => {
  switch (state) {
    case "listening":
      return "listening";
    case "transcribing":
    case "cleaning":
    case "pasting":
      return "working";
    case "pasted":
    case "cancelled":
      return "done";
    case "error":
      return "error";
    default:
      return null;
  }
};

/**
 * `flow:state` come lo mostra la barra. Il tocco singolo scartato del doppio
 * tocco (`cancelled` con detail `tap`) è un gesto normale: la barra torna a
 * riposo senza "Annullato".
 */
export const normalizeFlowState = <T extends { state: FlowStateName; detail: string | null }>(
  e: T,
): T => (e.state === "cancelled" && e.detail === "tap" ? { ...e, state: "idle" } : e);

/**
 * Stati di `flow:state` che appartengono a un Transform (detail `transform` o
 * `transform_unchanged`): la barra li mostra con l'evento `flow:transform`, non
 * come una dettatura.
 */
export const isTransformFlow = (detail: string | null | undefined): boolean =>
  detail === "transform" || detail === "transform_unchanged";

/** Chiave i18n dell'etichetta di ogni stato. */
export const flowStateLabelKey = (state: FlowStateName): string =>
  `flow.state.${state}`;

// ---------- Notifiche ----------

export type NoticeKind =
  | "mic_missing"
  | "mic_blocked"
  | "mic_silent"
  | "mic_error"
  | "asr_starting"
  | "asr_not_ready"
  | "cleanup_starting"
  | "cleanup_not_ready"
  | "network_offline"
  | "duration_warning";

/** Sezione dell'hub che risolve la notifica. Le chiavi sono quelle di `SECTIONS_CONFIG`. */
export type NoticeTarget = "settings" | "models";

export interface FlowNotice {
  kind: NoticeKind;
  severity: "warning" | "error";
  /** `null`: la notifica non ha una sezione dell'hub che la risolve (solo "Chiudi"). */
  target: NoticeTarget | null;
  detail: string | null;
  /** Valori per il testo i18n (per esempio `{ minutes }` dell'avviso di durata). */
  params?: Record<string, number>;
}

export interface NoticeInputs {
  /** Ultimo `recording-error` ricevuto e non ancora superato da una registrazione riuscita. */
  recordingError: { error_type: string; detail?: string | null } | null;
  /** Ultimo `asr:engine-state`; `null` se il backend non lo emette ancora. */
  asr: EngineState | null;
  /** Ultimo `cleanup:engine-state`; `null` se il backend non lo emette ancora. */
  cleanup: EngineState | null;
  qualityMode: boolean;
  /** Da quanto, in ascolto, il microfono resta sotto la soglia di silenzio (0 se non in ascolto). */
  silentForMs: number;
  /** Secondi registrati all'ultimo `asr:duration-warning` della dettatura in corso; `null` o assente se non è arrivato. */
  durationWarningSecs?: number | null;
  /** Tetto di registrazione da `settings.asr.max_recording_minutes`; `null` o assente se non letto. */
  maxRecordingMinutes?: number | null;
}

/**
 * Minuti del tetto per il testo dell'avviso di durata: dalle impostazioni; se
 * non sono leggibili, dai secondi dell'evento (che arriva un minuto prima del tetto).
 */
export const durationCapMinutes = (
  maxRecordingMinutes: number | null | undefined,
  warningSecs: number,
): number =>
  maxRecordingMinutes != null && maxRecordingMinutes > 0
    ? maxRecordingMinutes
    : Math.max(1, Math.round((warningSecs + 60) / 60));

/** Dopo quanto silenzio continuo in ascolto la barra avvisa "il microfono non sente niente". */
export const MIC_SILENT_AFTER_MS = 4000;

const isNetworkDetail = (detail: string | null | undefined): boolean =>
  typeof detail === "string" && detail.toLowerCase().startsWith("network");

/**
 * Classifica gli eventi del backend in notifiche per la barra e per la home.
 * Ordine: l'avviso di durata (la registrazione sta per fermarsi), poi gli errori
 * che bloccano la dettatura (microfono, ASR), poi la pulizia.
 */
export const classifyNotices = (input: NoticeInputs): FlowNotice[] => {
  const notices: FlowNotice[] = [];

  if (input.durationWarningSecs != null) {
    notices.push({
      kind: "duration_warning",
      severity: "warning",
      target: null,
      detail: null,
      params: {
        minutes: durationCapMinutes(input.maxRecordingMinutes, input.durationWarningSecs),
      },
    });
  }

  const err = input.recordingError;
  if (err) {
    const detail = err.detail ?? null;
    if (err.error_type === "no_input_device") {
      notices.push({ kind: "mic_missing", severity: "error", target: "settings", detail });
    } else if (err.error_type === "microphone_permission_denied") {
      notices.push({ kind: "mic_blocked", severity: "error", target: "settings", detail });
    } else {
      notices.push({ kind: "mic_error", severity: "error", target: "settings", detail });
    }
  } else if (input.silentForMs >= MIC_SILENT_AFTER_MS) {
    notices.push({ kind: "mic_silent", severity: "warning", target: "settings", detail: null });
  }

  const asr = input.asr;
  if (asr && asr.state !== "ready") {
    notices.push(
      asr.state === "starting"
        ? { kind: "asr_starting", severity: "warning", target: "models", detail: asr.detail }
        : { kind: "asr_not_ready", severity: "error", target: "models", detail: asr.detail },
    );
  }

  const cleanup = input.cleanup;
  if (cleanup) {
    if (input.qualityMode && isNetworkDetail(cleanup.detail)) {
      notices.push({
        kind: "network_offline",
        severity: "warning",
        target: "settings",
        detail: cleanup.detail,
      });
    } else if (cleanup.state !== "ready") {
      notices.push(
        cleanup.state === "starting"
          ? { kind: "cleanup_starting", severity: "warning", target: "models", detail: cleanup.detail }
          : { kind: "cleanup_not_ready", severity: "error", target: "models", detail: cleanup.detail },
      );
    }
  }

  return notices;
};

/** Soglia del livello medio (0..1) sotto cui il microfono si considera muto. */
export const MIC_SILENCE_LEVEL = 0.02;

export const isSilentFrame = (levels: number[]): boolean => {
  if (levels.length === 0) return true;
  const max = Math.max(...levels.map((v) => (Number.isFinite(v) ? v : 0)));
  return max < MIC_SILENCE_LEVEL;
};

// ---------- Posizione della barra ----------

export type DockEdge = "top" | "bottom" | "left" | "right";

export const DOCK_EDGES: DockEdge[] = ["top", "bottom", "left", "right"];

/** Normalizza `overlay_position`: oggi Rust conosce solo top e bottom, left e right arrivano con l'estensione dell'enum. */
export const toDockEdge = (value: unknown): DockEdge =>
  value === "top" || value === "left" || value === "right" ? value : "bottom";
