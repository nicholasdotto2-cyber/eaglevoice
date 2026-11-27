/**
 * Statistiche e raggruppamento della Home: cronologia per Oggi, Ieri,
 * data; parole totali; giorni di fila. Logica pura, provata con `bun test`.
 * I `timestamp` della cronologia sono in secondi.
 */

const DAY_MS = 86_400_000;

/** Mezzanotte locale del giorno di `ms`. */
const startOfDay = (ms: number): number => {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};

/** Giorni interi fra due mezzanotti locali (resiste al cambio dell'ora legale). */
const dayDiff = (a: number, b: number): number => Math.round((a - b) / DAY_MS);

export type DayKind = "today" | "yesterday" | "date";

export interface DayGroup<T> {
  /** Mezzanotte locale del giorno, in ms. */
  day: number;
  kind: DayKind;
  items: T[];
}

/** Raggruppa per giorno, dal più recente; dentro il giorno l'ordine resta quello di partenza. */
export const groupByDay = <T extends { timestamp: number }>(
  entries: T[],
  nowMs: number,
): DayGroup<T>[] => {
  const today = startOfDay(nowMs);
  const groups = new Map<number, T[]>();
  for (const e of entries) {
    const day = startOfDay(e.timestamp * 1000);
    const list = groups.get(day);
    if (list) list.push(e);
    else groups.set(day, [e]);
  }
  return [...groups.entries()]
    .sort((a, b) => b[0] - a[0])
    .map(([day, items]) => {
      const diff = dayDiff(today, day);
      return {
        day,
        kind: diff === 0 ? "today" : diff === 1 ? "yesterday" : "date",
        items,
      };
    });
};

/** Parole di un testo (sequenze separate da spazi). */
export const countWords = (text: string): number =>
  text.trim() ? text.trim().split(/\s+/).length : 0;

/**
 * Giorni di fila con almeno una dettatura, fino a oggi. Se oggi non c'è ancora
 * una dettatura, la serie che finisce ieri vale ancora (non si azzera a mezzanotte).
 */
export const streakDays = (timestampsSec: number[], nowMs: number): number => {
  const days = new Set(timestampsSec.map((t) => startOfDay(t * 1000)));
  let cursor = startOfDay(nowMs);
  if (!days.has(cursor)) cursor = startOfDay(cursor - DAY_MS / 2);
  let streak = 0;
  while (days.has(cursor)) {
    streak++;
    cursor = startOfDay(cursor - DAY_MS / 2);
  }
  return streak;
};

/** Saluto secondo l'ora: mattina fino alle 13, pomeriggio fino alle 18, poi sera. */
export const greetingKey = (hour: number): "morning" | "afternoon" | "evening" =>
  hour < 13 ? "morning" : hour < 18 ? "afternoon" : "evening";
