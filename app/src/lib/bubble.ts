/**
 * Logica pura della bolla di Eagle Voice: che forma prende (sfera o pillola),
 * come si dispone nella finestra secondo il bordo di aggancio, e il ciclo di
 * animazione dei fili, che gira solo quando serve. Niente React, niente Tauri:
 * si prova con `bun test`.
 */
import type { FlowStateName } from "./backend";
import type { DockEdge } from "./flow";

// ---------- Forma della bolla ----------

/**
 * Forme della bolla ("Orbita più Scia"):
 * - `sphere`: la sfera di vetro a riposo (anche quando c'è un avviso accanto).
 * - `voice`: pillola con i fili che seguono la voce (in ascolto).
 * - `orbit`: al rilascio i fili diventano tre orbite e la pillola si chiude in
 *   sfera; una cometa con la scia gira sull'anello (primo secondo di elaborazione).
 * - `linger`: oltre 1 s la sfera si allunga in una pillola corta con "Ancora un
 *   attimo", la cometa segue il bordo e compare la X per annullare.
 * - `pasted`: sfera con la spunta verde, lampo dell'anello e riflesso, una volta.
 * - `done`: pillola con la conferma breve (annullato).
 * - `error`: pillola con l'errore, quando non c'è un avviso che lo spiega.
 * - `transform`: pillola corta "Uso {nome}", poi "Sto sistemando il testo".
 * - `transformDone`: pillola corta "Fatto. Vedi modifiche" (apre il diff).
 */
export type BubbleView =
  | "sphere"
  | "voice"
  | "orbit"
  | "linger"
  | "pasted"
  | "done"
  | "error"
  | "transform"
  | "transformDone";

/** Dopo quanto l'elaborazione mostra "Ancora un attimo" e la X. */
export const LINGER_AFTER_MS = 1000;
/** Dopo quanto un Transform passa da "Uso {nome}" a "Sto sistemando il testo". */
export const TRANSFORM_SLOW_MS = 2000;
/** Quanto resta "Fatto. Vedi modifiche" (di più col mouse sopra). */
export const TRANSFORM_DONE_MS = 5000;
/** Quanto resta l'avviso "Imparato: {parola}" con Annulla. */
export const LEARNED_NOTICE_MS = 6000;

export interface BubbleExtras {
  /** Da quanto dura l'elaborazione (trascrizione, pulizia, incolla). */
  workingMs?: number;
  /** Transform in corso o appena finito, fuori dalla dettatura. */
  transform?: "running" | "done" | null;
}

export const bubbleView = (
  state: FlowStateName | null,
  noticeOpen: boolean,
  extras: BubbleExtras = {},
): BubbleView => {
  switch (state) {
    case "listening":
      return "voice";
    case "transcribing":
    case "cleaning":
    case "pasting":
      return (extras.workingMs ?? 0) >= LINGER_AFTER_MS ? "linger" : "orbit";
    case "pasted":
      return "pasted";
    case "cancelled":
      return noticeOpen ? "sphere" : "done";
    case "error":
      return noticeOpen ? "sphere" : "error";
    default:
      if (extras.transform === "running") return "transform";
      if (extras.transform === "done") return noticeOpen ? "sphere" : "transformDone";
      return "sphere";
  }
};

/** Forme a sfera: riposo, orbite, incollato. Le altre sono pillole. */
export const isSphereView = (view: BubbleView): boolean =>
  view === "sphere" || view === "orbit" || view === "pasted";

/** La pillola larga (280 px) della voce: sui bordi laterali l'avviso non le sta accanto. */
export const isWideView = (view: BubbleView): boolean => !isSphereView(view);

/** Misura in px della bolla per ogni forma: la tela dei fili prende la stessa. */
export const bubbleSize = (view: BubbleView): { w: number; h: number } => {
  switch (view) {
    case "voice":
      return { w: 280, h: 52 };
    case "orbit":
    case "pasted":
      return { w: 44, h: 44 };
    case "sphere":
      return { w: 38, h: 38 };
    case "transformDone":
    case "transform":
    case "done":
    case "error":
      return { w: 236, h: 40 };
    case "linger":
      return { w: 212, h: 40 };
  }
};

/**
 * Il ciclo `requestAnimationFrame` gira solo in ascolto, nelle orbite, in
 * "Ancora un attimo" e nel Transform in corso. A riposo, nelle conferme e con
 * `prefers-reduced-motion` nessun ciclo: la finestra resta aperta tutto il
 * giorno e un ciclo continuo consuma GPU.
 */
export const bubbleAnimates = (view: BubbleView, reducedMotion: boolean): boolean =>
  !reducedMotion &&
  (view === "voice" || view === "orbit" || view === "linger" || view === "transform");

/** Etichetta del Transform in corso: il nome, poi dopo 2 s una frase che rassicura. */
export const transformRunningKey = (runningMs: number): "using" | "slow" =>
  runningMs >= TRANSFORM_SLOW_MS ? "slow" : "using";

// ---------- Disposizione secondo il bordo ----------

export interface DockLayout {
  /** `flex-direction` del contenitore: l'avviso è il primo figlio, la bolla il secondo. */
  direction: "row" | "row-reverse" | "column" | "column-reverse";
  /** `justify-content`: porta la bolla contro il lato del bordo. */
  justify: "flex-start" | "flex-end";
  /** `align-items`: centra lungo il bordo, o attacca al lato quando l'avviso sta sopra la pillola. */
  align: "center" | "flex-start" | "flex-end";
  /** Dove sta l'avviso rispetto alla bolla. */
  noticeSide: "above" | "below" | "left" | "right";
}

/**
 * Disposizione dentro la finestra. Lungo il bordo il contenuto è centrato;
 * sull'asse perpendicolare sta attaccato al lato del bordo. L'avviso va verso il
 * centro dello schermo: sopra dal basso, sotto dall'alto, a destra da sinistra,
 * a sinistra da destra. Sui bordi laterali la pillola larga (280 px) non lascia
 * posto all'avviso nella finestra 360x190: lì l'avviso va sopra la pillola,
 * sempre attaccato al lato del bordo.
 */
export const dockLayout = (edge: DockEdge, wide = false): DockLayout => {
  switch (edge) {
    case "bottom":
      return { direction: "column", justify: "flex-end", align: "center", noticeSide: "above" };
    case "top":
      return { direction: "column-reverse", justify: "flex-end", align: "center", noticeSide: "below" };
    case "left":
      return wide
        ? { direction: "column", justify: "flex-end", align: "flex-start", noticeSide: "above" }
        : { direction: "row-reverse", justify: "flex-end", align: "center", noticeSide: "right" };
    case "right":
      return wide
        ? { direction: "column", justify: "flex-end", align: "flex-end", noticeSide: "above" }
        : { direction: "row", justify: "flex-end", align: "center", noticeSide: "left" };
  }
};

// ---------- Livello della voce ----------

/** Livello unico 0..1 dai secchi di `mic-level` (già lisciati): media dei più alti, con una curva che alza le voci basse. */
export const voiceLevel = (levels: number[]): number => {
  const v = levels.filter((x) => Number.isFinite(x)).map((x) => Math.max(0, Math.min(1, x)));
  if (v.length === 0) return 0;
  const top = [...v].sort((a, b) => b - a).slice(0, Math.max(1, Math.ceil(v.length / 3)));
  const mean = top.reduce((a, b) => a + b, 0) / top.length;
  return Math.min(1, Math.pow(mean, 0.7));
};

// ---------- Ciclo di animazione ----------

export interface FrameScheduler {
  request: (cb: (time: number) => void) => number;
  cancel: (id: number) => void;
}

export interface FrameLoop {
  /** Avvia o ferma il ciclo. Fermo, non resta nessuna richiesta in coda. */
  setActive: (active: boolean) => void;
  isRunning: () => boolean;
}

/** Ciclo `requestAnimationFrame` con un tetto di fotogrammi al secondo (60 di default). */
export const createFrameLoop = (
  draw: (time: number, dtMs: number) => void,
  scheduler: FrameScheduler,
  maxFps = 60,
): FrameLoop => {
  const interval = 1000 / maxFps;
  let id: number | null = null;
  let last: number | null = null;
  let lastDraw: number | null = null;
  const tick = (time: number) => {
    id = scheduler.request(tick);
    if (last !== null) {
      const elapsed = time - last;
      // Un millisecondo di tolleranza: a 60 Hz i fotogrammi arrivano ogni 16,7 ms circa.
      if (elapsed < interval - 1) return;
      // Il resto passa al fotogramma dopo, così a 144 Hz la media resta vicina a 60.
      last = elapsed >= interval ? time - (elapsed % interval) : time;
    } else {
      last = time;
    }
    const dt = lastDraw === null ? 0 : time - lastDraw;
    lastDraw = time;
    draw(time, dt);
  };
  return {
    setActive: (active) => {
      if (active && id === null) {
        last = null;
        lastDraw = null;
        id = scheduler.request(tick);
      } else if (!active && id !== null) {
        scheduler.cancel(id);
        id = null;
      }
    },
    isRunning: () => id !== null,
  };
};

/** Lo scheduler del browser. */
export const browserScheduler: FrameScheduler = {
  request: (cb) => window.requestAnimationFrame(cb),
  cancel: (id) => window.cancelAnimationFrame(id),
};
