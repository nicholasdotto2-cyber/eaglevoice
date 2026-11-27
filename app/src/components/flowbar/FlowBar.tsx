/**
 * Bolla di Eagle Voice (la barra di dettatura). A riposo una sfera di vetro; quando si
 * parla diventa una pillola con i fili di seta che seguono la voce. Al rilascio
 * ("Orbita più Scia") i fili si avvolgono in tre orbite, la pillola si
 * chiude in sfera e una cometa gira sull'anello; oltre 1 s la sfera si allunga
 * in "Ancora un attimo" con la X; all'incolla lampo dell'anello, riflesso e
 * spunta verde. Tempo, parole al minuto e i tasti stop e annulla compaiono al
 * passaggio del mouse sulla pillola della voce.
 *
 * Avvisi (microfono, motori, rete, incolla non riuscito, parola imparata, diff
 * del Transform) stanno in un riquadro di vetro accanto alla bolla, verso il
 * centro dello schermo.
 *
 * Clic e trascinamento li gestisce il Rust: al `pointerdown` sinistro sul corpo
 * della bolla si chiama solo `flowBarPress()`. La finestra non prende mai il
 * focus (`focusable(false)` in overlay.rs); i pulsanti bloccano il `mousedown`.
 */
import React, { useEffect, useMemo, useState } from "react";
import { COPIED_CLOSE_MS } from "@/lib/barSize";
import { useTranslation } from "react-i18next";
import { emit } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { commands } from "@/bindings";
import {
  FLOW_EVENTS,
  flowStop,
  retryTransform,
  undoLearnedWord,
  undoTransform,
  type BackendResult,
  type FlowStateEvent,
  type FlowTransformEvent,
  type HubNavigateEvent,
  type LearnedEvent,
} from "@/lib/backend";
import {
  flowStateLabelKey,
  isTransformFlow,
  formatElapsed,
  pastePreview,
  type DockEdge,
  type FlowNotice,
  type PasteFailedEvent,
} from "@/lib/flow";
import {
  bubbleAnimates,
  bubbleSize,
  bubbleView,
  dockLayout,
  isSphereView,
  isWideView,
  transformRunningKey,
  voiceLevel,
} from "@/lib/bubble";
import { countChanges, wordDiff } from "@/lib/diff";
import { copyToClipboard } from "@/components/settings/history/clipboard";
import { useReducedMotion } from "@/hooks/useReducedMotion";
import { SilkThreads } from "./SilkThreads";
import "./FlowBar.css";

/**
 * Copia negli appunti. Prima il plugin clipboard di Tauri, che non chiede il
 * focus (la finestra della bolla non lo prende mai); poi gli appunti del browser.
 */
export const copyFlowText = async (text: string): Promise<boolean> => {
  try {
    await writeText(text);
    return true;
  } catch (e) {
    console.warn("clipboard-manager non disponibile, provo navigator.clipboard:", e);
    return copyToClipboard(text);
  }
};

/** Misure della pillola della voce dentro la finestra 300x72 di overlay.rs. */
export const PILL_W = 280;
export const PILL_H = 52;

const noFocus = (e: React.MouseEvent) => e.preventDefault();
/** I pulsanti non devono avviare il trascinamento o il clic della bolla. */
const stopPress = (e: React.PointerEvent) => e.stopPropagation();

/** Apre l'hub sulla sezione che risolve l'avviso. */
export const openHubSection = async (section: string) => {
  try {
    await commands.showMainWindowCommand();
  } catch (e) {
    console.warn("show_main_window_command non riuscito:", e);
  }
  await emit(FLOW_EVENTS.hubNavigate, { section } satisfies HubNavigateEvent);
};

/** Pressione sul corpo della bolla: il Rust decide se è un clic o un trascinamento. */
const pressBubble = (e: React.PointerEvent) => {
  if (e.button !== 0) return;
  commands.flowBarPress().catch((err) => console.warn("flow_bar_press:", err));
};

interface IconButtonProps {
  label: string;
  onClick: () => void;
  variant?: "stop" | "cancel";
  children: React.ReactNode;
}

const BarButton: React.FC<IconButtonProps> = ({
  label,
  onClick,
  variant = "cancel",
  children,
}) => (
  <button
    type="button"
    className={`fb-btn fb-btn-${variant}`}
    aria-label={label}
    title={label}
    tabIndex={-1}
    onMouseDown={noFocus}
    onPointerDown={stopPress}
    onClick={onClick}
  >
    {children}
  </button>
);

/** Pulsante di testo dentro un avviso. */
const NoticeAction: React.FC<{
  onClick: () => void;
  primary?: boolean;
  children: React.ReactNode;
}> = ({ onClick, primary, children }) => (
  <button
    type="button"
    className={`fb-notice-action${primary ? " fb-notice-primary" : ""}`}
    tabIndex={-1}
    onMouseDown={noFocus}
    onPointerDown={stopPress}
    onClick={onClick}
  >
    {children}
  </button>
);

const StopIcon = () => (
  <svg viewBox="0 0 16 16" aria-hidden="true">
    <rect x="4" y="4" width="8" height="8" rx="1.5" fill="currentColor" />
  </svg>
);
const CancelIcon = () => (
  <svg viewBox="0 0 16 16" aria-hidden="true">
    <path
      d="M4 4 L12 12 M12 4 L4 12"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
    />
  </svg>
);
const CheckIcon = () => (
  <svg viewBox="0 0 16 16" aria-hidden="true">
    <path
      d="M3.5 8.5 L6.5 11.5 L12.5 4.5"
      stroke="currentColor"
      strokeWidth="1.8"
      fill="none"
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  </svg>
);
const AlertIcon = () => (
  <svg viewBox="0 0 16 16" aria-hidden="true">
    <path
      d="M8 3.5 V9"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
    />
    <circle cx="8" cy="12" r="1.1" fill="currentColor" />
  </svg>
);
const SparkIcon = () => (
  <svg viewBox="0 0 16 16" aria-hidden="true">
    <path
      d="M8 2.5 L9.3 6.7 L13.5 8 L9.3 9.3 L8 13.5 L6.7 9.3 L2.5 8 L6.7 6.7 Z"
      fill="currentColor"
    />
  </svg>
);

interface FlowNoticeRowProps {
  notice: FlowNotice;
  onDismiss: (kind: FlowNotice["kind"]) => void;
}

export const FlowNoticeRow: React.FC<FlowNoticeRowProps> = ({
  notice,
  onDismiss,
}) => {
  const { t } = useTranslation();
  return (
    <div
      className={`fb-card fb-notice fb-${notice.severity}`}
      role="status"
      title={notice.detail ?? undefined}
    >
      <span className="fb-notice-icon">
        <AlertIcon />
      </span>
      <span className="fb-notice-text">{t(`notices.${notice.kind}`, notice.params)}</span>
      <span className="fb-notice-actions">
        {notice.target && (
          <NoticeAction onClick={() => notice.target && openHubSection(notice.target)}>
            {t(`notices.action.${notice.target}`)}
          </NoticeAction>
        )}
        <BarButton
          label={t("flow.dismiss")}
          onClick={() => onDismiss(notice.kind)}
        >
          <CancelIcon />
        </BarButton>
      </span>
    </div>
  );
};

interface PasteFailedRowProps {
  event: PasteFailedEvent;
  pasteLastShortcut: string;
  onDismiss: () => void;
}

/** Incolla non riuscito: inizio del testo, "Copia" e il rimando a Incolla ultima. */
export const PasteFailedRow: React.FC<PasteFailedRowProps> = ({
  event,
  pasteLastShortcut,
  onDismiss,
}) => {
  const { t } = useTranslation();
  const [copied, setCopied] = useState<"idle" | "ok" | "fail">("idle");

  // Dopo la copia l'avviso mostra "Copiato" per un attimo e si chiude: la
  // chiusura riporta la bolla a riposo (flow_rest_bar in useFlowEvents).
  useEffect(() => {
    if (copied !== "ok") return;
    const id = setTimeout(onDismiss, COPIED_CLOSE_MS);
    return () => clearTimeout(id);
  }, [copied, onDismiss]);

  const handleCopy = async () => {
    setCopied((await copyFlowText(event.text)) ? "ok" : "fail");
  };

  return (
    <div
      className="fb-card fb-notice fb-paste-failed fb-error"
      role="alert"
      title={event.reason ?? undefined}
    >
      <span className="fb-notice-icon">
        <AlertIcon />
      </span>
      <span className="fb-paste-body">
        <span className="fb-paste-text">
          {t("flow.pasteFailed.title", { text: pastePreview(event.text) })}
        </span>
        <span className="fb-paste-hint">
          {t("flow.pasteFailed.hint", { shortcut: pasteLastShortcut })}
        </span>
      </span>
      <span className="fb-notice-actions">
        <NoticeAction primary onClick={handleCopy}>
          {copied === "ok"
            ? t("flow.pasteFailed.copied")
            : copied === "fail"
              ? t("flow.pasteFailed.copyFailed")
              : t("flow.pasteFailed.copy")}
        </NoticeAction>
        <BarButton label={t("flow.dismiss")} onClick={onDismiss}>
          <CancelIcon />
        </BarButton>
      </span>
    </div>
  );
};

/** Esito di un comando dentro un avviso: fatto, non collegato o errore. */
type ActionState = "idle" | "ok" | "notConnected" | "error";
const resultState = (res: BackendResult<unknown>): ActionState =>
  res.ok ? "ok" : res.notConnected ? "notConnected" : "error";

interface LearnedRowProps {
  event: LearnedEvent;
  onDismiss: () => void;
}

/** "Imparato: {giusta}" per 6 s con Annulla: Annulla toglie davvero la voce. */
export const LearnedRow: React.FC<LearnedRowProps> = ({ event, onDismiss }) => {
  const { t } = useTranslation();
  const [undo, setUndo] = useState<ActionState>("idle");

  useEffect(() => {
    if (undo !== "ok") return;
    const id = setTimeout(onDismiss, COPIED_CLOSE_MS);
    return () => clearTimeout(id);
  }, [undo, onDismiss]);

  return (
    <div className="fb-card fb-notice fb-learned" role="status">
      <span className="fb-notice-icon fb-learned-icon">
        <SparkIcon />
      </span>
      <span className="fb-paste-body">
        <span className="fb-notice-text">
          {t("flow.learned.title", { right: event.right })}
        </span>
        <span className="fb-paste-hint">
          {undo === "notConnected"
            ? t("flow.notConnected")
            : undo === "error"
              ? t("flow.learned.undoFailed")
              : t("flow.learned.detail", { wrong: event.wrong, right: event.right })}
        </span>
      </span>
      <span className="fb-notice-actions">
        <NoticeAction
          primary
          onClick={async () => setUndo(resultState(await undoLearnedWord(event.id)))}
        >
          {undo === "ok" ? t("flow.learned.undone") : t("flow.learned.undo")}
        </NoticeAction>
        <BarButton label={t("flow.dismiss")} onClick={onDismiss}>
          <CancelIcon />
        </BarButton>
      </span>
    </div>
  );
};

interface TransformDiffProps {
  event: FlowTransformEvent;
  onClose: () => void;
  /** Chiamato prima di `undo_transform`: il `done` che segue è l'Annulla. */
  onUndo?: () => void;
}

/** Diff del Transform (360x190): numero di modifiche, parti tolte e aggiunte, Annulla, Copia, Riprova. */
export const TransformDiffCard: React.FC<TransformDiffProps> = ({
  event,
  onClose,
  onUndo = () => {},
}) => {
  const { t } = useTranslation();
  const segments = useMemo(
    () => wordDiff(event.before, event.after),
    [event.before, event.after],
  );
  const changes = event.changes > 0 ? event.changes : countChanges(segments);
  const [status, setStatus] = useState<{
    what: string;
    state: ActionState;
    error?: string;
  } | null>(null);

  // In errore il messaggio di undo_transform e retry_transform si mostra così com'è.
  const run = async (what: string, action: () => Promise<BackendResult<unknown>>) => {
    const res = await action();
    setStatus({ what, state: resultState(res), error: res.ok ? undefined : res.error });
  };

  const copy = async () =>
    setStatus({
      what: "copy",
      state: (await copyFlowText(event.after)) ? "ok" : "error",
    });

  const statusText = (() => {
    if (!status) return null;
    if (status.state === "notConnected") return t("flow.notConnected");
    if (status.state === "error") return status.error || t("flow.diff.failed");
    return t(`flow.diff.done.${status.what}`);
  })();

  return (
    <div className="fb-card fb-notice fb-diff" role="dialog" aria-label={event.name}>
      <div className="fb-diff-head">
        <span className="fb-notice-text">
          {t("flow.diff.title", { count: changes, name: event.name })}
        </span>
        <BarButton label={t("flow.dismiss")} onClick={onClose}>
          <CancelIcon />
        </BarButton>
      </div>
      <div className="fb-diff-body">
        {segments.map((s, i) =>
          s.kind === "same" ? (
            <span key={i}>{s.text}</span>
          ) : s.kind === "del" ? (
            <del key={i} className="fb-diff-del">
              {s.text}
            </del>
          ) : (
            <ins key={i} className="fb-diff-ins">
              {s.text}
            </ins>
          ),
        )}
      </div>
      <div className="fb-diff-actions">
        <span className="fb-diff-status">{statusText}</span>
        <NoticeAction
          onClick={() => {
            onUndo();
            run("undo", undoTransform);
          }}
        >
          {t("flow.diff.undo")}
        </NoticeAction>
        <NoticeAction onClick={copy}>{t("flow.diff.copy")}</NoticeAction>
        <NoticeAction primary onClick={() => run("retry", retryTransform)}>
          {t("flow.diff.retry")}
        </NoticeAction>
      </div>
    </div>
  );
};

export interface FlowBarProps {
  flow: FlowStateEvent | null;
  levels: number[];
  /** Parole al minuto dell'ultima dettatura (da `flow:dictation-stats`). */
  lastWpm: number | null;
  /** Media della sessione (da `flow:dictation-stats`). */
  averageWpm: number | null;
  elapsedMs: number;
  /** Da quanto dura l'elaborazione dopo il rilascio (0 fuori dall'elaborazione). */
  workingMs?: number;
  notices: FlowNotice[];
  onDismissNotice: (kind: FlowNotice["kind"]) => void;
  pasteFailed?: PasteFailedEvent | null;
  onDismissPasteFailed?: () => void;
  pasteLastShortcut?: string;
  /** Parola appena imparata (`cleanup:learned`), con l'avviso di 6 s. */
  learned?: LearnedEvent | null;
  onDismissLearned?: () => void;
  /** Ultimo `flow:transform` ancora da mostrare. */
  transform?: FlowTransformEvent | null;
  /** Da quanto è partito il Transform in corso. */
  transformMs?: number;
  /** Il mouse sopra "Fatto. Vedi modifiche" tiene aperta la conferma. */
  onHoldTransform?: (hold: boolean) => void;
  onDismissTransform?: () => void;
  diffOpen?: boolean;
  onOpenDiff?: () => void;
  /** "p/m" accanto alla spunta all'incolla (impostazione, predefinito spento). */
  showPasteWpm?: boolean;
  /** L'ultimo `done` del Transform viene da un Annulla. */
  transformUndone?: boolean;
  onTransformUndo?: () => void;
  /** A riposo con la bolla sempre visibile. */
  idle?: boolean;
  transcribeShortcut?: string;
  edge: DockEdge;
  modelLoading?: boolean;
  /** Solo gli avvisi accanto alla sfera (la dettatura è a riposo). */
  noticesOnly?: boolean;
  onCancel: () => void;
}

export const FlowBar: React.FC<FlowBarProps> = ({
  flow: rawFlow,
  levels,
  lastWpm,
  averageWpm,
  elapsedMs,
  workingMs = 0,
  notices,
  onDismissNotice,
  pasteFailed = null,
  onDismissPasteFailed = () => {},
  pasteLastShortcut = "",
  learned = null,
  onDismissLearned = () => {},
  transform = null,
  transformMs = 0,
  onHoldTransform = () => {},
  onDismissTransform = () => {},
  diffOpen = false,
  onOpenDiff = () => {},
  showPasteWpm = false,
  transformUndone = false,
  onTransformUndo = () => {},
  transcribeShortcut = "",
  edge,
  modelLoading = false,
  noticesOnly = false,
  onCancel,
}) => {
  const { t } = useTranslation();
  const reducedMotion = useReducedMotion();
  // Gli stati di flow:state di un Transform (detail transform...) li racconta flow:transform.
  const flow = rawFlow && isTransformFlow(rawFlow.detail) ? null : rawFlow;

  // Un solo avviso alla volta: il diff aperto, il testo non incollato, la
  // parola imparata, il Transform fallito, poi il più grave (l'elenco completo
  // è nella home dell'hub).
  const unchanged = transform?.state === "done" && transform.changes === 0;
  const diff = diffOpen && transform?.state === "done" && !unchanged ? transform : null;
  const transformFailed = transform?.state === "error" ? transform : null;
  const notice = notices[0];
  const noticeOpen = Boolean(diff || pasteFailed || learned || transformFailed || notice);
  const transformPhase =
    transform?.state === "running" ? "running" : transform?.state === "done" ? "done" : null;
  const view = bubbleView(noticesOnly ? null : (flow?.state ?? null), noticeOpen, {
    workingMs,
    transform: transformPhase,
  });
  const wide = view === "voice";
  const layout = dockLayout(edge, wide);
  const level = view === "voice" ? voiceLevel(levels) : 0;
  const size = bubbleSize(view);
  const silk =
    view === "voice" || view === "orbit" || view === "linger" || view === "transform";

  const handleStop = async () => {
    const res = await flowStop();
    if (!res.ok) console.warn("flow_stop:", res.error);
  };

  const stateLabel = (() => {
    if (view === "transform" && transform)
      return transformRunningKey(transformMs) === "slow"
        ? t("flow.transform.slow")
        : t("flow.transform.using", { name: transform.name });
    if (view === "transformDone")
      return unchanged
        ? t(transformUndone ? "flow.transform.undone" : "flow.transform.unchanged")
        : t("flow.transform.done");
    if (!flow) return t("flow.idle.hint", { shortcut: transcribeShortcut });
    if (modelLoading && flow.state === "transcribing")
      return t("flow.state.loadingModel");
    if (flow.state === "error" && flow.detail)
      return t("flow.errorWithDetail", { detail: flow.detail });
    return t(flowStateLabelKey(flow.state));
  })();

  let content: React.ReactNode = null;
  if (view === "voice") {
    content = (
      <div className="eb-controls">
        <span className="fb-timer">{formatElapsed(elapsedMs)}</span>
        <span
          className="fb-wpm"
          title={
            averageWpm === null
              ? t("flow.wpmTitle")
              : t("flow.wpmSessionTitle", {
                  average: averageWpm,
                  last: lastWpm ?? "-",
                })
          }
        >
          {averageWpm === null ? "" : t("flow.wpm", { count: averageWpm })}
        </span>
        <span className="eb-spacer" />
        <BarButton label={t("flow.stop")} variant="stop" onClick={handleStop}>
          <StopIcon />
        </BarButton>
        <BarButton label={t("flow.cancel")} onClick={onCancel}>
          <CancelIcon />
        </BarButton>
      </div>
    );
  } else if (view === "linger") {
    content = (
      <div className="eb-short">
        <span className="eb-veil fb-label">{t("flow.linger")}</span>
        <BarButton label={t("flow.cancel")} onClick={onCancel}>
          <CancelIcon />
        </BarButton>
      </div>
    );
  } else if (view === "transform") {
    content = (
      <div className="eb-short">
        <span className="eb-veil fb-label">{stateLabel}</span>
      </div>
    );
  } else if (view === "transformDone") {
    content = (
      <button
        type="button"
        className="eb-short eb-short-button"
        tabIndex={-1}
        onMouseDown={noFocus}
        onPointerDown={stopPress}
        onClick={unchanged ? undefined : onOpenDiff}
        onMouseEnter={() => onHoldTransform(true)}
        onMouseLeave={() => onHoldTransform(false)}
      >
        <span className="fb-status-icon fb-check">
          <CheckIcon />
        </span>
        <span className="eb-veil fb-label">{stateLabel}</span>
      </button>
    );
  } else if (view === "pasted") {
    content = (
      <span className="eb-check" aria-hidden="true">
        <CheckIcon />
      </span>
    );
  } else if (view === "done" && flow) {
    content = (
      <div className={`eb-status fb-${flow.state}`}>
        <span className="fb-status-icon">
          <CancelIcon />
        </span>
        <span className="eb-veil fb-label">{stateLabel}</span>
      </div>
    );
  } else if (view === "error") {
    content = (
      <div className="eb-status fb-err">
        <span className="fb-status-icon">
          <AlertIcon />
        </span>
        <span className="eb-veil fb-label" title={flow?.detail ?? undefined}>
          {stateLabel}
        </span>
      </div>
    );
  }

  const sphereAlert =
    view === "sphere" && (flow?.state === "error" || noticeOpen);

  let card: React.ReactNode = null;
  if (diff)
    card = (
      <TransformDiffCard event={diff} onClose={onDismissTransform} onUndo={onTransformUndo} />
    );
  else if (pasteFailed)
    card = (
      <PasteFailedRow
        event={pasteFailed}
        pasteLastShortcut={pasteLastShortcut}
        onDismiss={onDismissPasteFailed}
      />
    );
  else if (learned) card = <LearnedRow event={learned} onDismiss={onDismissLearned} />;
  else if (transformFailed)
    card = (
      <div className="fb-card fb-notice fb-error" role="alert">
        <span className="fb-notice-icon">
          <AlertIcon />
        </span>
        <span className="fb-notice-text">
          {transformFailed.message || t("flow.transform.error", { name: transformFailed.name })}
        </span>
        <BarButton label={t("flow.dismiss")} onClick={onDismissTransform}>
          <CancelIcon />
        </BarButton>
      </div>
    );
  else if (notice) card = <FlowNoticeRow notice={notice} onDismiss={onDismissNotice} />;

  return (
    <div
      className={`fb-stage fb-edge-${edge}`}
      data-notice-side={layout.noticeSide}
      style={{
        flexDirection: layout.direction,
        justifyContent: layout.justify,
        alignItems: layout.align,
      }}
    >
      {card}
      <div className="eb-row">
        <div
          className={`eb-bubble eb-${view}${isWideView(view) ? " eb-pill" : ""}${
            sphereAlert ? " eb-alert" : ""
          }`}
          data-state={flow?.state ?? "idle"}
          data-view={view}
          role="button"
          aria-label={stateLabel}
          title={isSphereView(view) ? stateLabel : undefined}
          style={{ width: size.w, height: size.h }}
          onMouseDown={noFocus}
          onPointerDown={pressBubble}
        >
          {silk && (
            <span className="eb-silk-clip">
              <SilkThreads
                mode={view === "voice" ? "voice" : "orbit"}
                comet={view !== "voice"}
                level={level}
                active={bubbleAnimates(view, reducedMotion)}
                reducedMotion={reducedMotion}
                width={size.w}
                height={size.h}
              />
            </span>
          )}
          {view === "pasted" && <span className="eb-flash" aria-hidden="true" />}
          <span className="eb-shine" aria-hidden="true" />
          {content}
        </div>
        {view === "pasted" && showPasteWpm && lastWpm !== null && (
          <span className="eb-wpm-tag" title={t("flow.wpmLastTitle")}>
            {t("flow.wpm", { count: lastWpm })}
          </span>
        )}
      </div>
    </div>
  );
};
