/**
 * Azioni su una dettatura della cronologia: copia, riprova, elimina.
 * Ricevono i comandi dei bindings come parametro, così i test li sostituiscono con un mock.
 */
import type { commands } from "@/bindings";
import { historyTexts, type FlowHistoryEntry } from "@/lib/history";
import { copyToClipboard, type ClipboardWriter } from "./clipboard";

export type HistoryCommands = Pick<
  typeof commands,
  "deleteHistoryEntry" | "retryHistoryEntryTranscription" | "archiveHistoryEntry"
>;

/** Copia il testo finale (corretto a mano, poi pulito, poi grezzo). */
export const copyHistoryEntry = (
  entry: FlowHistoryEntry,
  clipboard?: ClipboardWriter,
): Promise<boolean> => copyToClipboard(historyTexts(entry).final, clipboard);

/** Rilancia la trascrizione dell'audio salvato; lancia un errore se il backend rifiuta. */
export const retryHistoryEntry = async (
  cmds: HistoryCommands,
  id: number,
): Promise<void> => {
  const result = await cmds.retryHistoryEntryTranscription(id);
  if (result.status !== "ok") throw new Error(String(result.error));
};

/**
 * Archivia (`archived = true`) o riporta nella lista (`false`) una dettatura.
 * Lancia un errore se il backend rifiuta; la voce aggiornata arriva poi con
 * `history-update-payload` `{ action: "updated", entry }`.
 */
export const archiveHistoryEntry = async (
  cmds: HistoryCommands,
  id: number,
  archived: boolean,
): Promise<void> => {
  const result = await cmds.archiveHistoryEntry(id, archived);
  if (result.status !== "ok") throw new Error(String(result.error));
};

/** Elimina la dettatura e il suo audio; false se il backend non ci riesce. */
export const deleteHistoryEntry = async (
  cmds: HistoryCommands,
  id: number,
): Promise<boolean> => {
  try {
    const result = await cmds.deleteHistoryEntry(id);
    return result.status === "ok";
  } catch (error) {
    console.error("Failed to delete entry:", error);
    return false;
  }
};
