/**
 * Testata della Home: l'aquila a punti
 * quadrati bianchi che batte le ali, inquadrata da linee a mirino, un testo
 * verticale piccolo, nuvole e stelle generate (SVG, niente immagini).
 *
 * L'aquila parte dal poligono `AQUILA` (lib/eagle.ts): a ogni fotogramma le ali
 * prendono una posa interpolata e la sagoma si ricampiona nella griglia di
 * punti. Il ciclo (30 fotogrammi al secondo) si ferma quando la finestra non è
 * visibile (`document.hidden`). All'apertura i punti si compongono in 1,1 s.
 * Con `prefers-reduced-motion`: aquila ferma, nessuna animazione.
 */
import React, { useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { browserScheduler, createFrameLoop } from "@/lib/bubble";
import {
  dotGrid,
  flapPose,
  FLIGHT_REGION,
  hash01,
  wingLift,
} from "@/lib/eagle";
import { useReducedMotion } from "@/hooks/useReducedMotion";

const CANVAS_W = 230;
const CANVAS_H = 250;
/** Pixel per unità del riquadro dell'aquila. */
const SCALE = 2.2;
const STEP = 3.3;
const FLAP_MS = 1700;
/** Durata della composizione iniziale (al massimo 1,2 s). */
const COMPOSE_MS = 1100;

const ORIGIN_X = (CANVAS_W - 100 * SCALE) / 2;
const ORIGIN_Y = (CANVAS_H - (FLIGHT_REGION[3] - FLIGHT_REGION[1]) * SCALE) / 2;

/** Pochissimi punti colorati sull'aquila. */
const ACCENT_DOTS: Record<string, string> = {
  "17-12": "#E5484D",
  "14-17": "#3FD3B5",
  "13-20": "#E5484D",
};

const easeOut = (t: number) => 1 - Math.pow(1 - t, 3);

/** Disegna un fotogramma dell'aquila. `compose` 0..1: i punti arrivano da posizioni sparse. */
export const drawEagle = (
  ctx: CanvasRenderingContext2D,
  phase: number,
  compose: number,
) => {
  ctx.clearRect(0, 0, CANVAS_W, CANVAS_H);
  const { cells } = dotGrid(flapPose(wingLift(phase)), STEP, FLIGHT_REGION);
  const cell = STEP * SCALE;
  const dot = cell * 0.72;
  cells.forEach(([c, r], i) => {
    let x = ORIGIN_X + c * cell + (cell - dot) / 2;
    let y = ORIGIN_Y + r * cell + (cell - dot) / 2;
    let alpha = 1;
    if (compose < 1) {
      // Ogni punto parte con un ritardo e da una posizione sparsa, stabili.
      const seed = c * 131 + r * 17;
      const delay = hash01(seed) * 0.45;
      const p = easeOut(Math.min(1, Math.max(0, (compose - delay) / 0.55)));
      const dx = (hash01(seed + 1) - 0.5) * 160;
      const dy = (hash01(seed + 2) - 0.5) * 160;
      x += dx * (1 - p);
      y += dy * (1 - p);
      alpha = p;
      if (alpha <= 0) return;
    }
    ctx.globalAlpha = alpha;
    ctx.fillStyle = ACCENT_DOTS[`${c}-${r}`] ?? (i % 97 === 0 ? "#DDE0FF" : "#FFFFFF");
    ctx.fillRect(Math.round(x), Math.round(y), dot, dot);
  });
  ctx.globalAlpha = 1;
};

/** Nuvole bianche morbide: ellissi sovrapposte e sfocate (SVG generato). */
const Clouds: React.FC = () => (
  <svg
    className="absolute inset-0 w-full h-full pointer-events-none"
    viewBox="0 0 680 240"
    preserveAspectRatio="xMidYMid slice"
    aria-hidden="true"
  >
    <defs>
      <filter id="ev-cloud-blur" x="-30%" y="-60%" width="160%" height="220%">
        <feGaussianBlur stdDeviation="9" />
      </filter>
      <filter id="ev-cloud-soft" x="-30%" y="-60%" width="160%" height="220%">
        <feGaussianBlur stdDeviation="4" />
      </filter>
    </defs>
    <g filter="url(#ev-cloud-blur)" fill="#FFFFFF" opacity="0.55">
      <ellipse cx="80" cy="214" rx="90" ry="26" />
      <ellipse cx="150" cy="200" rx="60" ry="30" />
      <ellipse cx="560" cy="60" rx="70" ry="20" />
      <ellipse cx="610" cy="48" rx="44" ry="22" />
    </g>
    <g filter="url(#ev-cloud-soft)" fill="#FFFFFF" opacity="0.8">
      <ellipse cx="128" cy="206" rx="38" ry="16" />
      <ellipse cx="96" cy="214" rx="30" ry="11" />
      <ellipse cx="588" cy="56" rx="28" ry="10" />
    </g>
  </svg>
);

/** Stelle: puntini bianchi, due colorati. Posizioni stabili. */
const STARS = Array.from({ length: 14 }, (_, i) => ({
  x: 4 + hash01(i * 7 + 3) * 92,
  y: 6 + hash01(i * 11 + 5) * 86,
  s: hash01(i * 13 + 1) > 0.75 ? 2 : 1.5,
  color: i === 3 ? "#E5484D" : i === 9 ? "#3FD3B5" : "#FFFFFF",
}));

export const EagleHero: React.FC = () => {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const reducedMotion = useReducedMotion();
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const dpr = Math.max(1, window.devicePixelRatio || 1);
    canvas.width = Math.round(CANVAS_W * dpr);
    canvas.height = Math.round(CANVAS_H * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    if (reducedMotion) {
      drawEagle(ctx, 0.25, 1);
      return;
    }

    let start: number | null = null;
    const loop = createFrameLoop(
      (time) => {
        if (start === null) start = time;
        const t = time - start;
        drawEagle(ctx, t / FLAP_MS, Math.min(1, t / COMPOSE_MS));
      },
      browserScheduler,
      30,
    );
    // Si ferma quando la finestra è nascosta; alla riapertura dell'hub i punti
    // si ricompongono (l'animazione di avvio).
    const onVisibility = () => {
      if (document.hidden) {
        loop.setActive(false);
      } else {
        start = null;
        loop.setActive(true);
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    loop.setActive(!document.hidden);
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      loop.setActive(false);
    };
  }, [reducedMotion]);

  return (
    <div
      className="ev-grid ev-corners relative h-[260px] rounded-[14px] border border-hairline overflow-hidden"
      data-testid="eagle-hero"
    >
      <Clouds />
      {STARS.map((s, i) => (
        <span
          key={i}
          aria-hidden="true"
          className="absolute"
          style={{
            left: `${s.x}%`,
            top: `${s.y}%`,
            width: s.s,
            height: s.s,
            background: s.color,
            opacity: s.color === "#FFFFFF" ? 0.7 : 1,
          }}
        />
      ))}
      {/* Linee a mirino che inquadrano l'aquila. */}
      <span
        aria-hidden="true"
        className="absolute left-0 right-0 h-px bg-white/45"
        style={{ top: 214 }}
      />
      <span
        aria-hidden="true"
        className="absolute left-0 right-0 h-px bg-white/25"
        style={{ top: 30 }}
      />
      <span
        aria-hidden="true"
        className="absolute top-0 bottom-0 w-px bg-white/45"
        style={{ left: "calc(50% + 112px)" }}
      />
      <span
        aria-hidden="true"
        className="absolute top-0 bottom-0 w-px bg-white/25"
        style={{ left: "calc(50% - 150px)" }}
      />
      <canvas
        ref={canvasRef}
        className="absolute"
        style={{
          width: CANVAS_W,
          height: CANVAS_H,
          left: `calc(50% - ${CANVAS_W / 2}px)`,
          top: 4,
        }}
        role="img"
        aria-label="Eagle Voice"
      />
      <span
        className="absolute font-mono text-[10px] tracking-[0.18em] text-white"
        style={{
          left: "calc(50% + 118px)",
          top: 92,
          writingMode: "vertical-rl",
        }}
      >
        {`EAGLE VOICE · v${version || "0.9.8"}`}
      </span>
    </div>
  );
};
