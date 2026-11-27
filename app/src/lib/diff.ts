/**
 * Differenza a parole fra il testo prima e dopo un Transform, per il riquadro
 * "Vedi modifiche" della barra. Logica pura, provata con `bun test`.
 */

export type DiffKind = "same" | "del" | "ins";

export interface DiffSegment {
  kind: DiffKind;
  text: string;
}

/** Parole con lo spazio che le segue, così ricomporre il testo non perde nulla. */
const tokens = (text: string): string[] => text.match(/\S+\s*/g) ?? [];

const norm = (token: string) => token.trimEnd();

/**
 * Sottosequenza comune più lunga sulle parole (programmazione dinamica). Oltre
 * 3000 parole per lato (il tetto dei Transform) il costo resta sotto 9 milioni
 * di celle; sopra, il testo intero risulta tolto e aggiunto.
 */
export const wordDiff = (before: string, after: string): DiffSegment[] => {
  const a = tokens(before);
  const b = tokens(after);
  const out: DiffSegment[] = [];
  const push = (kind: DiffKind, text: string) => {
    const last = out[out.length - 1];
    if (last && last.kind === kind) last.text += text;
    else out.push({ kind, text });
  };
  if (a.length * b.length > 9_000_000) {
    if (before) push("del", before);
    if (after) push("ins", after);
    return out;
  }
  const n = a.length;
  const m = b.length;
  const lcs: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] =
        norm(a[i]) === norm(b[j])
          ? lcs[i + 1][j + 1] + 1
          : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (norm(a[i]) === norm(b[j])) {
      push("same", b[j]);
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      push("del", a[i++]);
    } else {
      push("ins", b[j++]);
    }
  }
  while (i < n) push("del", a[i++]);
  while (j < m) push("ins", b[j++]);
  return out;
};

/** Modifiche: gruppi contigui di parole tolte o aggiunte (una sostituzione conta una). */
export const countChanges = (segments: DiffSegment[]): number => {
  let changes = 0;
  let inChange = false;
  for (const s of segments) {
    if (s.kind === "same") inChange = false;
    else if (!inChange) {
      changes++;
      inChange = true;
    }
  }
  return changes;
};
