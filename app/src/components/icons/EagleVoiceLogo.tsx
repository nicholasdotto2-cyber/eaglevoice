import React, { useMemo } from "react";
import { AQUILA, dotGrid } from "@/lib/eagle";

/**
 * Segno di Eagle Voice: aquila a punti quadrati bianchi su un quadrato
 * arrotondato blu cobalto #3039BB, con due linee sottili a mirino. Stessa
 * geometria di `src-tauri\icons\eagle-voice.svg` (viewBox 1024, rx 230; aquila
 * nel riquadro dal 10 % al 90 % in orizzontale e dal 12 % al 92 % in verticale).
 * Sotto i 40 px i punti non si leggono: l'aquila è una sagoma piena, come nel tray.
 */

const VIEW = 1024;
const RIQ = { x0: 0.1 * VIEW, y0: 0.12 * VIEW, side: 0.8 * VIEW };
/** Il poligono occupa y 9..81: la griglia parte più in alto per centrarlo. */
const REGION = [0, -5, 100, 95] as const;
const CELLS = 26;

const toView = (x: number, y: number) =>
  [RIQ.x0 + (x / 100) * RIQ.side, RIQ.y0 + ((y - REGION[1]) / 100) * RIQ.side] as const;

export const EagleVoiceMark: React.FC<{
  size?: number;
  className?: string;
  // Le icone della barra laterale passano width e height.
  width?: number | string;
  height?: number | string;
}> = ({ size, width, height, className = "" }) => {
  const w = size ?? width ?? 32;
  const h = size ?? height ?? w;
  const px = typeof w === "number" ? w : 32;
  const dots = px >= 40;
  const cells = useMemo(() => (dots ? dotGrid(AQUILA, 100 / CELLS, REGION).cells : []), [dots]);
  const step = RIQ.side / CELLS;
  const silhouette = useMemo(
    () => AQUILA.map(([x, y]) => toView(x, y).map((v) => v.toFixed(1)).join(",")).join(" "),
    [],
  );
  return (
    <svg
      width={w}
      height={h}
      viewBox={`0 0 ${VIEW} ${VIEW}`}
      className={className}
      aria-hidden="true"
      xmlns="http://www.w3.org/2000/svg"
    >
      <rect width={VIEW} height={VIEW} rx="230" fill="#3039BB" />
      {dots && (
        <g stroke="#FFFFFF" strokeOpacity="0.43" strokeWidth="4">
          <line x1="82" y1="717" x2="942" y2="717" />
          <line x1="758" y1="102" x2="758" y2="922" />
        </g>
      )}
      {dots ? (
        <g fill="#FFFFFF">
          {cells.map(([c, r]) => (
            <rect
              key={`${c}-${r}`}
              x={RIQ.x0 + c * step + step * 0.14}
              y={RIQ.y0 + r * step + step * 0.14}
              width={step * 0.72}
              height={step * 0.72}
            />
          ))}
        </g>
      ) : (
        <polygon points={silhouette} fill="#FFFFFF" />
      )}
    </svg>
  );
};

/**
 * Logo completo: segno più la scritta "Eagle Voice". `size` è il lato del
 * segno in pixel; la scritta si adatta. `stacked` mette la scritta sotto.
 */
const EagleVoiceLogo: React.FC<{
  size?: number;
  stacked?: boolean;
  className?: string;
}> = ({ size = 32, stacked = false, className = "" }) => (
  <div
    className={`flex items-center ${stacked ? "flex-col gap-3" : "gap-2.5"} ${className}`}
    role="img"
    aria-label="Eagle Voice"
  >
    <EagleVoiceMark
      size={size}
      className="shrink-0 rounded-[22%] ring-1 ring-white/35"
    />
    <span
      className={`flex ${stacked ? "flex-row gap-1.5" : "flex-col"} leading-none tracking-tight text-text`}
      style={{ fontSize: Math.round(size * (stacked ? 0.5 : 0.47)) }}
    >
      {/* eslint-disable-next-line i18next/no-literal-string */}
      <span className="font-semibold">Eagle</span>
      {/* eslint-disable-next-line i18next/no-literal-string */}
      <span className="font-normal text-mid-gray">Voice</span>
    </span>
  </div>
);

export default EagleVoiceLogo;
