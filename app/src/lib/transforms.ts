/**
 * Regole della pagina Transforms: quanti slot, quando compaiono gli
 * esempi, validazione dell'editor, scorciatoia letta da un tasto premuto.
 * Logica pura, provata con `bun test`.
 */
import type { TransformSlot } from "./backend";

/** "Crea nuovo" sparisce quando gli slot sono 9. */
export const MAX_SLOTS = 9;
export const MAX_EXAMPLES = 5;
export const EXAMPLE_MIN_WORDS = 50;
export const EXAMPLE_MAX_WORDS = 500;

export interface TransformDraft {
  slot: number;
  name: string;
  shortcut: string;
  prompt: string;
  examples: string[];
  builtin: boolean;
}

export const canCreate = (slots: Pick<TransformSlot, "slot">[]): boolean =>
  slots.length < MAX_SLOTS;

/** Primo slot libero per un personalizzato (dal 3 al 9: binding `transform_3`..`transform_9`). */
export const nextFreeSlot = (slots: Pick<TransformSlot, "slot">[]): number => {
  const used = new Set(slots.map((s) => s.slot));
  let n = 3;
  while (used.has(n)) n++;
  return n;
};


export const emptyDraft = (slots: Pick<TransformSlot, "slot">[]): TransformDraft => {
  const slot = nextFreeSlot(slots);
  // La scorciatoia vera è il binding `transform_N` (vuoto finché non la scegli).
  return { slot, name: "", shortcut: "", prompt: "", examples: [], builtin: false };
};

/** Gli esempi compaiono quando nome, scorciatoia e prompt sono tutti compilati. */
export const examplesUnlocked = (d: Pick<TransformDraft, "name" | "shortcut" | "prompt">): boolean =>
  d.name.trim() !== "" && d.shortcut.trim() !== "" && d.prompt.trim() !== "";

export const wordCount = (text: string): number =>
  text.trim() ? text.trim().split(/\s+/).length : 0;

export const exampleOk = (text: string): boolean => {
  const n = wordCount(text);
  return n >= EXAMPLE_MIN_WORDS && n <= EXAMPLE_MAX_WORDS;
};

export type TransformError = "name" | "prompt" | "tooManyExamples" | "exampleLength";

/**
 * Primo errore dell'editor, o null se si può salvare. Gli esempi vuoti non
 * contano. La scorciatoia non si valida qui: è un binding (`polish`,
 * `transform_N`) e i conflitti li controlla il comando delle scorciatoie.
 */
export const validateTransform = (d: TransformDraft): TransformError | null => {
  if (!d.name.trim()) return "name";
  if (!d.builtin && !d.prompt.trim()) return "prompt";
  const examples = d.examples.filter((e) => e.trim());
  if (examples.length > MAX_EXAMPLES) return "tooManyExamples";
  if (examples.some((e) => !exampleOk(e))) return "exampleLength";
  return null;
};

/** Il Transform da salvare: esempi vuoti tolti, spazi ai bordi tolti. */
export const draftToSlot = (d: TransformDraft): TransformSlot => ({
  slot: d.slot,
  name: d.name.trim(),
  shortcut: d.shortcut.trim(),
  prompt: d.prompt.trim(),
  examples: d.examples.map((e) => e.trim()).filter(Boolean),
  builtin: d.builtin,
});

