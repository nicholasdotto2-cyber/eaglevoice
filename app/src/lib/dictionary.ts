/** Filtro, ricerca e ordinamento del dizionario: logica pura, provata con `bun test`. */
import type { DictionaryEntry } from "./backend";

export type DictionaryKind = "word" | "replacement" | "snippet";
export type DictionaryFilter = "all" | DictionaryKind | "starred" | "learned";
export type DictionarySort = "frequency" | "recent" | "alpha";

export const entryKind = (entry: Pick<DictionaryEntry, "is_snippet" | "replacement">): DictionaryKind => {
  if (entry.is_snippet) return "snippet";
  if (entry.replacement && entry.replacement.trim().length > 0) return "replacement";
  return "word";
};

/** Minuscolo e senza accenti: "perché" trova "perche". */
export const normalizeForSearch = (text: string): string =>
  text
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .trim();

export const matchesQuery = (entry: DictionaryEntry, query: string): boolean => {
  const q = normalizeForSearch(query);
  if (q.length === 0) return true;
  return (
    normalizeForSearch(entry.phrase).includes(q) ||
    normalizeForSearch(entry.replacement ?? "").includes(q)
  );
};

export const matchesFilter = (entry: DictionaryEntry, filter: DictionaryFilter): boolean => {
  switch (filter) {
    case "all":
      return true;
    case "starred":
      return entry.starred;
    case "learned":
      return entry.source === "learned";
    default:
      return entryKind(entry) === filter;
  }
};

const lastUsedTime = (entry: DictionaryEntry): number => {
  if (!entry.last_used) return 0;
  const t = Date.parse(entry.last_used);
  return Number.isNaN(t) ? 0 : t;
};

/** Le voci con la stella vengono sempre prima; poi l'ordinamento scelto. */
export const compareEntries = (sort: DictionarySort) => (a: DictionaryEntry, b: DictionaryEntry): number => {
  if (a.starred !== b.starred) return a.starred ? -1 : 1;
  if (sort === "frequency" && a.frequency !== b.frequency) return b.frequency - a.frequency;
  if (sort === "recent") {
    const diff = lastUsedTime(b) - lastUsedTime(a);
    if (diff !== 0) return diff;
  }
  return a.phrase.localeCompare(b.phrase, "it", { sensitivity: "base" });
};

export const filterDictionary = (
  entries: DictionaryEntry[],
  query: string,
  filter: DictionaryFilter = "all",
  sort: DictionarySort = "frequency",
): DictionaryEntry[] =>
  entries
    .filter((e) => matchesFilter(e, filter) && matchesQuery(e, query))
    .sort(compareEntries(sort));

export interface DictionaryDraft {
  kind: DictionaryKind;
  phrase: string;
  replacement: string;
}

/** Valida una voce scritta a mano. Restituisce la chiave i18n dell'errore, o `null` se va bene. */
export const validateDraft = (draft: DictionaryDraft, existing: DictionaryEntry[], editingId?: string): string | null => {
  const phrase = draft.phrase.trim();
  if (phrase.length === 0) return "dictionary.errors.emptyPhrase";
  if (draft.kind !== "word" && draft.replacement.trim().length === 0) {
    return "dictionary.errors.emptyReplacement";
  }
  const key = normalizeForSearch(phrase);
  const duplicate = existing.some((e) => e.id !== editingId && normalizeForSearch(e.phrase) === key);
  return duplicate ? "dictionary.errors.duplicate" : null;
};
