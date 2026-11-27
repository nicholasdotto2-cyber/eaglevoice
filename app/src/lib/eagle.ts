/**
 * L'aquila di Eagle Voice: il poligono di `tools\genera-icone.py` (riquadro
 * 100x100), le pose delle ali e la griglia di punti quadrati. Logica pura, la
 * disegnano `EagleVoiceLogo` (segno fermo) e `EagleHero` (aquila che vola).
 */

export type Point = readonly [number, number];

/** Aquila in volo vista da sotto: stesso poligono di `AQUILA` in `tools\genera-icone.py`. */
export const AQUILA: readonly Point[] = [
  [42, 26], [45, 21], [41, 19], [35, 18], [40, 15], [46, 11], [52, 13], [55, 19],
  [58, 26], [68, 20], [80, 14], [92, 9], [99, 11], [93, 15], [99, 18], [92, 21],
  [98, 25], [90, 27], [94, 32], [85, 33], [76, 38], [66, 44], [60, 48], [60, 58],
  [66, 76], [58, 74], [50, 81], [42, 74], [34, 76], [40, 58], [40, 48], [34, 44],
  [24, 38], [15, 33], [6, 32], [10, 27], [2, 25], [8, 21], [1, 18], [7, 15],
  [1, 11], [8, 9], [20, 14], [32, 20],
];

/** Ray casting: il punto sta dentro il poligono? */
export const insidePolygon = (poly: readonly Point[], x: number, y: number): boolean => {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i];
    const [xj, yj] = poly[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) {
      inside = !inside;
    }
  }
  return inside;
};

/** Spalle: dentro questa distanza dal centro del corpo (x = 50) le ali non si muovono. */
const SHOULDER = 9;

/**
 * Ali piegate di `lift` unità del riquadro alla punta: negativo in su, positivo
 * in giù. Le ali alzate si accorciano un poco, come viste di scorcio.
 */
export const flapPose = (lift: number, poly: readonly Point[] = AQUILA): Point[] =>
  poly.map(([x, y]) => {
    const d = Math.abs(x - 50);
    if (d <= SHOULDER) return [x, y] as const;
    const k = (d - SHOULDER) / (50 - SHOULDER);
    const nx = 50 + (x - 50) * (1 - 0.14 * Math.abs(lift / 24) * k);
    return [nx, y + lift * Math.pow(k, 1.3)] as const;
  });

/** Pose del battito: in alto, in mezzo, in basso, in mezzo (quattro, interpolate). */
export const WING_POSES = [-16, 0, 14, 0] as const;

const smooth = (t: number) => t * t * (3 - 2 * t);

/** Sollevamento delle ali alla fase `phase` (0..1) del battito. */
export const wingLift = (phase: number): number => {
  const n = WING_POSES.length;
  const p = (((phase % 1) + 1) % 1) * n;
  const i = Math.floor(p);
  const a = WING_POSES[i % n];
  const b = WING_POSES[(i + 1) % n];
  return a + (b - a) * smooth(p - i);
};

export interface DotGrid {
  /** Celle piene, in coordinate di griglia (colonna, riga). */
  cells: Array<[number, number]>;
  cols: number;
  rows: number;
}

/**
 * Ricampiona il poligono in una griglia di punti quadrati: una cella è piena se
 * il suo centro sta dentro la sagoma. `region` è la parte del riquadro coperta
 * dalla griglia (x0, y0, x1, y1); `step` il lato della cella in unità del riquadro.
 */
export const dotGrid = (
  poly: readonly Point[],
  step: number,
  region: readonly [number, number, number, number] = [0, 0, 100, 100],
): DotGrid => {
  const [x0, y0, x1, y1] = region;
  const cols = Math.max(1, Math.round((x1 - x0) / step));
  const rows = Math.max(1, Math.round((y1 - y0) / step));
  const cells: Array<[number, number]> = [];
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) {
      const x = x0 + (c + 0.5) * step;
      const y = y0 + (r + 0.5) * step;
      if (insidePolygon(poly, x, y)) cells.push([c, r]);
    }
  }
  return { cells, cols, rows };
};

/** Riquadro dei punti del segno fermo (stesso margine dell'icona: 10 % ai lati). */
export const MARK_REGION = [0, 8, 100, 82] as const;

/** Riquadro dei punti dell'aquila che vola: lascia spazio alle ali alzate e abbassate. */
export const FLIGHT_REGION = [0, -14, 100, 100] as const;

/** Numero pseudo casuale stabile da un intero (per le stelle e la composizione iniziale). */
export const hash01 = (n: number): number => {
  const s = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return s - Math.floor(s);
};
