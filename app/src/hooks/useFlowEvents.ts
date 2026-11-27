/**
 * Eventi della barra di dettatura: stato della dettatura, livello del microfono,
 * errori di registrazione, stato dei due motori, incolla non riuscito, avviso
 * di durata e parole al minuto (da `flow:dictation-stats`). Calcola timer e
 * notifiche. Se il backend non emette ancora `flow:state`, la barra resta
 * sull'overlay di Handy (vedi `RecordingOverlay`).
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { commands } from "@/bindings";
import {
  FLOW_EVENTS,
  flowExpandBar,
  flowRestBar,
  type CleanupSettings,
  type EngineState,
  type FlowStateEvent,
  type FlowStateName,
  type FlowTransformEvent,
  type LearnedEvent,
} from "@/lib/backend";
import {
  LEARNED_NOTICE_MS,
  LINGER_AFTER_MS,
  TRANSFORM_DONE_MS,
  TRANSFORM_SLOW_MS,
} from "@/lib/bubble";
import {
  classifyNotices,
  normalizeFlowState,
  compactShortcut,
  isSilentFrame,
  type FlowNotice,
  type PasteFailedEvent,
} from "@/lib/flow";
import { formatKeyCombination } from "@/lib/utils/keyboard";
import type { RecordingErrorEvent } from "@/lib/types/events";
import { FLOW_SHORTCUTS } from "@/components/hub/shortcuts";
import { useDictationStats } from "./useDictationStats";
import { createBarSizer, type BarSizer } from "@/lib/barSize";

/** Scorciatoie che la barra cita: dettatura (suggerimento a riposo) e Incolla ultima. */
export interface FlowBarShortcuts {
  transcribe: string;
  pasteLast: string;
}

export interface FlowEventsState {
  /** Ultimo `flow:state`; `null` finché il backend non ne manda uno. */
  flow: FlowStateEvent | null;
  elapsedMs: number;
  /** Parole al minuto dell'ultima dettatura incollata. */
  lastWpm: number | null;
  /** Media della sessione. */
  averageWpm: number | null;
  notices: FlowNotice[];
  dismissNotice: (kind: FlowNotice["kind"]) => void;
  /** Ultimo `flow:paste-failed` non ancora chiuso. */
  pasteFailed: PasteFailedEvent | null;
  dismissPasteFailed: () => void;
  shortcuts: FlowBarShortcuts;
  /** Da quanto dura l'elaborazione dopo il rilascio (0 fuori). */
  workingMs: number;
  /** Ultima parola imparata, per 6 s. */
  learned: LearnedEvent | null;
  dismissLearned: () => void;
  /** Ultimo Transform da mostrare (in corso, finito per 5 s, fallito). */
  transform: FlowTransformEvent | null;
  transformMs: number;
  holdTransform: (hold: boolean) => void;
  dismissTransform: () => void;
  diffOpen: boolean;
  openDiff: () => void;
  /** "p/m" accanto alla spunta all'incolla (`flow_bar_paste_wpm`). */
  showPasteWpm: boolean;
  /** L'ultimo Transform finito è l'effetto di un Annulla. */
  transformUndone: boolean;
  /** Il diff ha chiesto Annulla: il prossimo `done` rimette il testo di prima. */
  markTransformUndo: () => void;
}


const isWorking = (s: FlowStateName | undefined) =>
  s === "transcribing" || s === "cleaning" || s === "pasting";

/** Quanto resta visibile la conferma "Incollato" o "Annullato" prima che la barra torni a riposo. */
export const DONE_LINGER_MS = 1400;

const fallbackShortcut = (id: string): string =>
  FLOW_SHORTCUTS.find((s) => s.id === id)?.defaultCombo ?? "";

export const useFlowEvents = (): FlowEventsState => {
  const [flow, setFlow] = useState<FlowStateEvent | null>(null);
  const [listenStart, setListenStart] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [recordingError, setRecordingError] =
    useState<RecordingErrorEvent | null>(null);
  const [asr, setAsr] = useState<EngineState | null>(null);
  const [cleanup, setCleanup] = useState<EngineState | null>(null);
  const [qualityMode, setQualityMode] = useState(false);
  const [silentSince, setSilentSince] = useState<number | null>(null);
  const [dismissed, setDismissed] = useState<Set<string>>(new Set());
  const [pasteFailed, setPasteFailed] = useState<PasteFailedEvent | null>(null);
  const [durationWarningSecs, setDurationWarningSecs] = useState<number | null>(
    null,
  );
  const [shortcuts, setShortcuts] = useState<FlowBarShortcuts>({
    transcribe: fallbackShortcut("transcribe"),
    pasteLast: fallbackShortcut("paste_last"),
  });
  /** Ultimo stato della finestra: `show-overlay` ("idle", "notice", "recording"...) o "hidden". */
  const [overlayMode, setOverlayMode] = useState("hidden");
  /** Tetto di registrazione (`asr.max_recording_minutes`), per il testo dell'avviso di durata. */
  const [maxRecordingMinutes, setMaxRecordingMinutes] = useState<number | null>(
    null,
  );
  const listeningRef = useRef(false);
  const stats = useDictationStats();
  const [workingStart, setWorkingStart] = useState<number | null>(null);
  const [learned, setLearned] = useState<LearnedEvent | null>(null);
  const [transform, setTransform] = useState<FlowTransformEvent | null>(null);
  const [transformStart, setTransformStart] = useState<number | null>(null);
  const [transformHeld, setTransformHeld] = useState(false);
  const [diffOpen, setDiffOpen] = useState(false);
  /** L'ultimo `done` arriva da un Annulla del diff (testo di prima rimesso). */
  const [transformUndone, setTransformUndone] = useState(false);
  const undoPendingRef = useRef(false);
  const markTransformUndo = useCallback(() => {
    undoPendingRef.current = true;
  }, []);
  const [pasteWpmSetting, setPasteWpmSetting] = useState<boolean | null>(null);

  const readSettings = useCallback(async () => {
    try {
      const res = await commands.getAppSettings();
      if (res.status === "ok") {
        const data = res.data as typeof res.data & {
          cleanup?: Partial<CleanupSettings>;
          asr?: { max_recording_minutes?: number };
          flow_bar_paste_wpm?: boolean;
        };
        setQualityMode(data.cleanup?.quality_mode ?? false);
        setPasteWpmSetting(
          typeof data.flow_bar_paste_wpm === "boolean" ? data.flow_bar_paste_wpm : null,
        );
        setMaxRecordingMinutes(data.asr?.max_recording_minutes ?? null);
        const binding = (id: string) => {
          const b = data.bindings?.[id]?.current_binding;
          return compactShortcut(
            b ? formatKeyCombination(b, "windows") : fallbackShortcut(id),
          );
        };
        setShortcuts({
          transcribe: binding("transcribe"),
          pasteLast: binding("paste_last"),
        });
      }
    } catch {
      // Impostazioni non leggibili: si resta sui valori precedenti.
    }
  }, []);

  useEffect(() => {
    readSettings();
    const unlisteners: Array<Promise<() => void>> = [
      listen<FlowStateEvent>(FLOW_EVENTS.state, (event) => {
        const payload = normalizeFlowState(event.payload);
        setFlow(payload);
        if (isWorking(payload.state)) {
          setWorkingStart((prev) => prev ?? Date.now());
        } else {
          setWorkingStart(null);
        }
        if (payload.state === "listening") {
          if (!listeningRef.current) {
            setListenStart(Date.now());
            setSilentSince(Date.now());
            setRecordingError(null);
            setDismissed(new Set());
            setPasteFailed(null);
            setDurationWarningSecs(null);
            readSettings();
          }
          listeningRef.current = true;
        } else {
          listeningRef.current = false;
          setSilentSince(null);
          // L'avviso di durata vale solo mentre si registra.
          setDurationWarningSecs(null);
          // A riposo la barra rilegge le scorciatoie per il suggerimento.
          if (payload.state === "idle") readSettings();
        }
      }),
      listen<number[]>(FLOW_EVENTS.micLevel, (event) => {
        if (!listeningRef.current) return;
        const silent = isSilentFrame(event.payload ?? []);
        setSilentSince((prev) => (silent ? (prev ?? Date.now()) : null));
      }),
      listen<RecordingErrorEvent>(FLOW_EVENTS.recordingError, (event) => {
        setRecordingError(event.payload);
        setDismissed(new Set());
      }),
      listen("recording-ready", () => setRecordingError(null)),
      listen<EngineState>(FLOW_EVENTS.asrEngineState, (event) =>
        setAsr(event.payload),
      ),
      listen<EngineState>(FLOW_EVENTS.cleanupEngineState, (event) =>
        setCleanup(event.payload),
      ),
      listen<PasteFailedEvent>(FLOW_EVENTS.pasteFailed, (event) => {
        if (event.payload?.text) {
          setPasteFailed({
            text: event.payload.text,
            reason: event.payload.reason ?? null,
          });
        }
      }),
      listen<string>("show-overlay", (event) => setOverlayMode(event.payload)),
      listen("hide-overlay", () => setOverlayMode("hidden")),
      listen<LearnedEvent>(FLOW_EVENTS.learned, (event) => {
        if (event.payload?.right) setLearned(event.payload);
      }),
      listen<FlowTransformEvent>(FLOW_EVENTS.transform, (event) => {
        const payload = event.payload;
        if (!payload?.state) return;
        // Dopo Annulla il backend manda `done` con changes 0 e i testi invertiti.
        setTransformUndone(undoPendingRef.current && payload.state === "done");
        if (payload.state !== "running") undoPendingRef.current = false;
        setTransform(payload);
        setDiffOpen(false);
        setTransformHeld(false);
        if (payload.state === "running") setTransformStart(Date.now());
      }),
      listen<{ seconds: number }>(FLOW_EVENTS.durationWarning, (event) => {
        setDurationWarningSecs(event.payload?.seconds ?? 0);
        setDismissed((prev) => {
          const next = new Set(prev);
          next.delete("duration_warning");
          return next;
        });
      }),
    ];
    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, [readSettings]);

  const listening = flow?.state === "listening";

  // Il timer avanza solo in ascolto.
  useEffect(() => {
    if (!listening) return;
    const id = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(id);
  }, [listening]);

  // "Incollato" e "Annullato" sono conferme brevi: poi la barra torna a riposo.
  useEffect(() => {
    if (flow?.state !== "pasted" && flow?.state !== "cancelled") return;
    const id = setTimeout(
      () => setFlow((f) => (f === flow ? { ...f, state: "idle" } : f)),
      DONE_LINGER_MS,
    );
    return () => clearTimeout(id);
  }, [flow]);

  // Elaborazione: un solo risveglio a 1 s per "Ancora un attimo".
  useEffect(() => {
    if (workingStart === null) return;
    const left = LINGER_AFTER_MS - (Date.now() - workingStart);
    if (left <= 0) return;
    const id = setTimeout(() => setNow(Date.now()), left + 10);
    return () => clearTimeout(id);
  }, [workingStart]);

  // Transform in corso: un risveglio a 2 s per "Sto sistemando il testo".
  useEffect(() => {
    if (transform?.state !== "running" || transformStart === null) return;
    const left = TRANSFORM_SLOW_MS - (Date.now() - transformStart);
    if (left <= 0) return;
    const id = setTimeout(() => setNow(Date.now()), left + 10);
    return () => clearTimeout(id);
  }, [transform, transformStart]);

  // "Fatto. Vedi modifiche" per 5 s; col mouse sopra o col diff aperto resta.
  useEffect(() => {
    if (transform?.state !== "done" || transformHeld || diffOpen) return;
    const id = setTimeout(() => setTransform(null), TRANSFORM_DONE_MS);
    return () => clearTimeout(id);
  }, [transform, transformHeld, diffOpen]);

  // "Imparato: {parola}" per 6 s.
  useEffect(() => {
    if (!learned) return;
    const id = setTimeout(() => setLearned(null), LEARNED_NOTICE_MS);
    return () => clearTimeout(id);
  }, [learned]);


  const elapsedMs = listenStart !== null ? Math.max(0, now - listenStart) : 0;
  const workingMs =
    workingStart !== null ? Math.max(0, Math.max(now, Date.now()) - workingStart) : 0;
  const transformMs =
    transform?.state === "running" && transformStart !== null
      ? Math.max(0, Date.now() - transformStart)
      : 0;

  const notices = useMemo(
    () =>
      classifyNotices({
        recordingError,
        asr,
        cleanup,
        qualityMode,
        silentForMs: listening && silentSince !== null ? now - silentSince : 0,
        durationWarningSecs,
        maxRecordingMinutes,
      }).filter((n) => !dismissed.has(n.kind)),
    [
      recordingError,
      asr,
      cleanup,
      qualityMode,
      listening,
      silentSince,
      now,
      dismissed,
      durationWarningSecs,
      maxRecordingMinutes,
    ],
  );

  const dismissNotice = useCallback((kind: FlowNotice["kind"]) => {
    setDismissed((prev) => new Set(prev).add(kind));
  }, []);

  const dismissPasteFailed = useCallback(() => setPasteFailed(null), []);

  // Misura della barra fuori dettatura: piena con una notifica aperta, a riposo
  // quando l'ultima si chiude (flow_expand_bar / flow_rest_bar).
  const sizerRef = useRef<BarSizer | null>(null);
  if (sizerRef.current === null) {
    sizerRef.current = createBarSizer({
      expand: () =>
        flowExpandBar().then((res) => {
          if (!res.ok) console.warn("flow_expand_bar:", res.error);
        }),
      rest: () => {
        // Se la barra si nasconde il backend non manda "idle": l'errore
        // dell'incolla non deve ricomparire alla prossima notifica.
        setFlow((f) => (f?.state === "error" ? { ...f, state: "idle" } : f));
        return flowRestBar().then((res) => {
          if (!res.ok) console.warn("flow_rest_bar:", res.error);
        });
      },
    });
  }

  useEffect(() => {
    if (overlayMode === "notice") sizerRef.current?.markExpanded();
    // "idle" e "recording": overlay.rs chiude l'avviso e rimette la misura dello stato.
    else if (
      overlayMode === "idle" ||
      overlayMode === "recording" ||
      overlayMode === "hidden"
    )
      sizerRef.current?.markRested();
  }, [overlayMode]);

  const dismissLearned = useCallback(() => setLearned(null), []);
  const dismissTransform = useCallback(() => {
    setTransform(null);
    setDiffOpen(false);
  }, []);
  const openDiff = useCallback(() => setDiffOpen(true), []);

  // Il Transform gira fuori dalla dettatura, con la finestra a riposo (52x52):
  // anche lui chiede la misura piena, come un avviso.
  const openNotices =
    notices.length + (pasteFailed ? 1 : 0) + (learned ? 1 : 0) + (transform ? 1 : 0);
  useEffect(() => {
    sizerRef.current?.update({ openNotices });
  }, [openNotices, overlayMode]);

  return {
    flow,
    elapsedMs,
    lastWpm: stats.lastWpm,
    averageWpm: stats.averageWpm,
    notices,
    dismissNotice,
    pasteFailed,
    dismissPasteFailed,
    shortcuts,
    workingMs,
    learned,
    dismissLearned,
    transform,
    transformMs,
    holdTransform: setTransformHeld,
    dismissTransform,
    diffOpen,
    openDiff,
    showPasteWpm: pasteWpmSetting ?? false,
    transformUndone,
    markTransformUndo,
  };
};
