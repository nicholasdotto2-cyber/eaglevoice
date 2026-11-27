/** Testi di una voce di cronologia, con i campi aggiunti da eagleVoice quando ci sono. */
import type { HistoryEntry } from "@/bindings";
import type { HistoryExtraFields } from "./backend";
import { normalizeForSearch } from "./dictionary";

export type FlowHistoryEntry = HistoryEntry & HistoryExtraFields;

export interface HistoryTexts {
  raw: string;
  cleaned: string | null;
  edited: string | null;
  /** Il testo da copiare: corretto a mano, poi pulito, poi grezzo. */
  final: string;
}

export const historyTexts = (e: FlowHistoryEntry): HistoryTexts => {
  const raw = e.raw_text ?? e.transcription_text;
  const cleaned = e.cleaned_text ?? e.post_processed_text ?? null;
  const edited = e.edited_text ?? null;
  return { raw, cleaned, edited, final: edited || cleaned || raw };
};

/** Vista della cronologia: la lista principale o le dettature archiviate. */
export type HistoryView = "list" | "archived";

/** True se la voce appartiene alla vista: le archiviate non stanno nella lista principale. */
export const inHistoryView = (
  e: Pick<HistoryEntry, "archived">,
  view: HistoryView,
): boolean => !!e.archived === (view === "archived");

export const matchesHistory = (
  e: FlowHistoryEntry,
  query: string,
  app: string | null,
): boolean => {
  if (app !== null && (e.app_name ?? "") !== app) return false;
  const q = normalizeForSearch(query);
  if (!q) return true;
  const { raw, cleaned, edited } = historyTexts(e);
  return [raw, cleaned, edited, e.app_name]
    .filter((x): x is string => typeof x === "string")
    .some((x) => normalizeForSearch(x).includes(q));
};
