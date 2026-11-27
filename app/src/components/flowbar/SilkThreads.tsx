/**
 * Fili di seta dentro la pillola: 7 curve
 * morbide con fase e frequenza diverse, bagliore bianco e cobalto chiaro,
 * fusione additiva. In ascolto seguono la voce (ampiezza e luce dal livello del
 * microfono); dopo il rilascio si avvolgono in tre orbite ("Orbita più Scia")
 * e una cometa con la scia gira sull'anello di vetro.
 *
 * Il ciclo `requestAnimationFrame` gira solo mentre `active` è vero e al
 * massimo a 60 fotogrammi al secondo. Con `reducedMotion` niente ciclo né
 * cometa: un disegno fermo, rifatto solo quando cambia il livello o la forma.
 */
import React, { useEffect, useRef } from "react";
import { browserScheduler, createFrameLoop } from "@/lib/bubble";

export type SilkMode = "voice" | "orbit";

interface SilkThreadsProps {
  mode: SilkMode;
  /** Cometa sull'anello (orbite, "Ancora un attimo", Transform in corso). */
  comet?: boolean;
  /** Livello della voce 0..1. */
  level: number;
  /** Il ciclo di animazione gira solo con `active`. */
  active: boolean;
  reducedMotion: boolean;
  width: number;
  height: number;
}

const THREADS = 7;
const COLORS = ["255, 255, 255", "140, 148, 255", "185, 190, 255"];
const TAU = Math.PI * 2;

const setupCanvas = (canvas: HTMLCanvasElement, w: number, h: number) => {
  const dpr = Math.max(1, window.devicePixelRatio || 1);
  if (canvas.width !== Math.round(w * dpr)) canvas.width = Math.round(w * dpr);
  if (canvas.height !== Math.round(h * dpr)) canvas.height = Math.round(h * dpr);
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return ctx;
};

const strokeThread = (
  ctx: CanvasRenderingContext2D,
  pts: Array<[number, number]>,
  rgb: string,
  alpha: number,
  /** Sfuma i capi del filo lungo la pillola (i fili della voce si toccano ai bordi). */
  fadeWidth = 0,
) => {
  // Tre passate: un bagliore largo, un alone tenue, poi il filo sottile e luminoso.
  for (const [width, a] of [
    [7, alpha * 0.07],
    [3.2, alpha * 0.22],
    [1.25, alpha],
  ] as const) {
    ctx.beginPath();
    pts.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
    ctx.lineWidth = width;
    if (fadeWidth > 0) {
      const g = ctx.createLinearGradient(0, 0, fadeWidth, 0);
      g.addColorStop(0, `rgba(${rgb}, 0)`);
      g.addColorStop(0.22, `rgba(${rgb}, ${a})`);
      g.addColorStop(0.78, `rgba(${rgb}, ${a})`);
      g.addColorStop(1, `rgba(${rgb}, 0)`);
      ctx.strokeStyle = g;
    } else {
      ctx.strokeStyle = `rgba(${rgb}, ${a})`;
    }
    ctx.stroke();
  }
};

const drawVoice = (
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  t: number,
  level: number,
) => {
  const cy = h / 2;
  for (let i = 0; i < THREADS; i++) {
    const f = 1.1 + i * 0.31;
    const speed = 0.0012 + i * 0.00021;
    const phase = i * 1.7;
    const amp = h * (0.07 + 0.34 * level) * (0.6 + 0.4 * Math.sin(i * 2.1 + 1));
    const alpha = Math.min(1, 0.32 + 0.68 * level);
    const pts: Array<[number, number]> = [];
    for (let x = 0; x <= w; x += 3) {
      const u = x / w;
      const env = Math.pow(Math.sin(Math.PI * u), 1.4);
      const wave =
        0.72 * Math.sin(TAU * f * u + t * speed + phase) +
        0.28 * Math.sin(TAU * f * 1.9 * u - t * speed * 1.3 + phase * 2);
      pts.push([x, cy + amp * env * wave]);
    }
    strokeThread(ctx, pts, COLORS[i % COLORS.length], alpha, w);
  }
};

/** Quanto dura l'avvolgimento dei fili in orbite dopo il rilascio. */
const MORPH_MS = 200;

/** Centro delle orbite: al centro nella sfera, a sinistra nella pillola corta. */
const orbitCenter = (w: number, h: number): [number, number] =>
  w > h + 4 ? [h / 2 + 2, h / 2] : [w / 2, h / 2];

/**
 * Tre orbite ellittiche inclinate di 60 gradi che ruotano. `morph` va da 0 a 1
 * nei primi 200 ms: i fili orizzontali della voce si stringono e si avvolgono.
 */
const drawOrbits = (
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  t: number,
  morph: number,
) => {
  const [cx, cy] = orbitCenter(w, h);
  const r = Math.min(w, h) * 0.34;
  const ease = 1 - Math.pow(1 - morph, 3);
  for (let k = 0; k < 3; k++) {
    const tilt = (k * Math.PI) / 3 + t * 0.0011 * (k % 2 === 0 ? 1 : -1);
    const rx = r + (w * 0.42 - r) * (1 - ease);
    const ry = r * (0.08 + 0.32 * ease);
    const cos = Math.cos(tilt * ease);
    const sin = Math.sin(tilt * ease);
    const pts: Array<[number, number]> = [];
    for (let s = 0; s <= TAU + 0.01; s += TAU / 64) {
      const x = rx * Math.cos(s);
      const y = ry * Math.sin(s);
      pts.push([cx + x * cos - y * sin, cy + x * sin + y * cos]);
    }
    strokeThread(ctx, pts, COLORS[k % COLORS.length], 0.7 - k * 0.08);
  }
  // Il nucleo: un punto luminoso al centro delle orbite.
  const g = ctx.createRadialGradient(cx, cy, 0, cx, cy, r * 0.45);
  g.addColorStop(0, "rgba(255, 255, 255, 0.9)");
  g.addColorStop(1, "rgba(185, 190, 255, 0)");
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(cx, cy, r * 0.45, 0, TAU);
  ctx.fill();
};

/** Punto sul bordo di una pillola (o di un cerchio, se w = h) a distanza `s` lungo il perimetro. */
const ringPoint = (w: number, h: number, s: number): [number, number] => {
  const inset = 1.6;
  const r = h / 2 - inset;
  const straight = Math.max(0, w - h);
  const arc = Math.PI * r;
  const per = 2 * straight + 2 * arc;
  let d = ((s % per) + per) % per;
  const left = h / 2;
  const right = h / 2 + straight;
  if (d < straight) return [left + d, inset];
  d -= straight;
  if (d < arc) {
    const a = -Math.PI / 2 + d / r;
    return [right + r * Math.cos(a), h / 2 + r * Math.sin(a)];
  }
  d -= arc;
  if (d < straight) return [right - d, h - inset];
  d -= straight;
  const a = Math.PI / 2 + d / r;
  return [left + r * Math.cos(a), h / 2 + r * Math.sin(a)];
};

const ringLength = (w: number, h: number) =>
  2 * Math.max(0, w - h) + 2 * Math.PI * (h / 2 - 1.6);

/** Cometa con la scia sull'anello di vetro: un giro ogni 1,1 s nella sfera, più lento nella pillola. */
const drawComet = (
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  t: number,
) => {
  const per = ringLength(w, h);
  const speed = per / (w > h + 4 ? 1900 : 1100);
  const head = t * speed;
  const trail = Math.min(per * 0.3, 60);
  const steps = 26;
  for (let i = steps; i >= 1; i--) {
    const a = ringPoint(w, h, head - (trail * i) / steps);
    const b = ringPoint(w, h, head - (trail * (i - 1)) / steps);
    const k = 1 - i / steps;
    ctx.beginPath();
    ctx.moveTo(a[0], a[1]);
    ctx.lineTo(b[0], b[1]);
    ctx.lineWidth = 0.6 + 2.2 * k;
    ctx.strokeStyle = `rgba(255, 255, 255, ${0.05 + 0.75 * k * k})`;
    ctx.stroke();
  }
  const [hx, hy] = ringPoint(w, h, head);
  const g = ctx.createRadialGradient(hx, hy, 0, hx, hy, 5);
  g.addColorStop(0, "rgba(255, 255, 255, 1)");
  g.addColorStop(1, "rgba(200, 205, 255, 0)");
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(hx, hy, 5, 0, TAU);
  ctx.fill();
};

/** Senza movimento: una linea luminosa che si alza con la voce. */
const drawStill = (
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  level: number,
) => {
  const y = h * 0.72 - level * h * 0.44;
  const pts: Array<[number, number]> = [
    [w * 0.14, y],
    [w * 0.86, y],
  ];
  strokeThread(ctx, pts, COLORS[0], 0.5 + 0.4 * level, w);
};


const paint = (canvas: HTMLCanvasElement, w: number, h: number) => {
  const ctx = setupCanvas(canvas, w, h);
  if (!ctx) return null;
  ctx.globalCompositeOperation = "source-over";
  ctx.clearRect(0, 0, w, h);
  ctx.globalCompositeOperation = "lighter";
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  return ctx;
};

export const SilkThreads: React.FC<SilkThreadsProps> = ({
  mode,
  comet = false,
  level,
  active,
  reducedMotion,
  width,
  height,
}) => {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const targetRef = useRef(level);
  const smoothRef = useRef(level);
  const modeRef = useRef(mode);
  const cometRef = useRef(comet);
  /** Istante in cui sono comparse le orbite, per l'avvolgimento dei primi 200 ms. */
  const orbitSinceRef = useRef<number | null>(null);
  targetRef.current = level;
  cometRef.current = comet;
  if (modeRef.current !== mode) {
    modeRef.current = mode;
    orbitSinceRef.current = null;
  }

  // Ciclo animato: parte con `active`, si ferma (e cancella la richiesta in coda)
  // appena la bolla torna ferma o il componente sparisce.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || reducedMotion || !active) return;
    const loop = createFrameLoop((time, dt) => {
      const ctx = paint(canvas, width, height);
      if (!ctx) return;
      const k = Math.min(1, dt / 90);
      smoothRef.current += (targetRef.current - smoothRef.current) * k;
      if (modeRef.current === "orbit") {
        if (orbitSinceRef.current === null) orbitSinceRef.current = time;
        const morph = Math.min(1, (time - orbitSinceRef.current) / MORPH_MS);
        drawOrbits(ctx, width, height, time, morph);
        if (cometRef.current) drawComet(ctx, width, height, time);
      } else {
        drawVoice(ctx, width, height, time, smoothRef.current);
      }
    }, browserScheduler);
    loop.setActive(true);
    return () => loop.setActive(false);
  }, [active, reducedMotion, width, height]);

  // Senza movimento: un solo disegno per ogni livello o forma nuova, nessun ciclo.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || (!reducedMotion && active)) return;
    const ctx = paint(canvas, width, height);
    if (!ctx) return;
    if (mode === "orbit") drawOrbits(ctx, width, height, 0, 1);
    else drawStill(ctx, width, height, level);
  }, [reducedMotion, active, level, mode, width, height]);

  return (
    <canvas
      ref={canvasRef}
      className="eb-silk"
      style={{ width, height }}
      aria-hidden="true"
    />
  );
};
