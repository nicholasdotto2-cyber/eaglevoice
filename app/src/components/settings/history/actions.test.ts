import { describe, expect, test } from "bun:test";
import type { HistoryEntry } from "@/bindings";
import {
  archiveHistoryEntry,
  copyHistoryEntry,
  deleteHistoryEntry,
  retryHistoryEntry,
  type HistoryCommands,
} from "./actions";

const entry = (over: Partial<HistoryEntry> = {}): HistoryEntry => ({
  id: 7,
  file_name: "flow-7.wav",
  timestamp: 1760800000,
  saved: false,
  title: "",
  transcription_text: "parola uno parola due parola tre",
  post_processed_text: "Parola uno, parola due parola tre.",
  post_process_prompt: null,
  post_process_requested: true,
  ...over,
});

/** Mock dei bindings: registra le chiamate e risponde come Tauri. */
const mockCommands = (status: "ok" | "error" = "ok") => {
  const calls: Array<[string, number, boolean?]> = [];
  const reply = async () =>
    status === "ok"
      ? ({ status: "ok", data: null } as const)
      : ({ status: "error", error: "voce non trovata" } as const);
  const cmds: HistoryCommands = {
    deleteHistoryEntry: async (id) => {
      calls.push(["delete_history_entry", id]);
      return reply();
    },
    retryHistoryEntryTranscription: async (id) => {
      calls.push(["retry_history_entry_transcription", id]);
      return reply();
    },
    archiveHistoryEntry: async (id, archived) => {
      calls.push(["archive_history_entry", id, archived]);
      return reply();
    },
  };
  return { cmds, calls };
};

describe("cronologia: copia, riprova, elimina", () => {
  test("copia il testo pulito, o quello grezzo se manca", async () => {
    const written: string[] = [];
    const clip = { writeText: async (t: string) => void written.push(t) };
    expect(await copyHistoryEntry(entry(), clip)).toBe(true);
    expect(
      await copyHistoryEntry(entry({ post_processed_text: null }), clip),
    ).toBe(true);
    expect(written).toEqual([
      "Parola uno, parola due parola tre.",
      "parola uno parola due parola tre",
    ]);
  });

  test("riprova chiama retry_history_entry_transcription e segnala il rifiuto", async () => {
    const ok = mockCommands("ok");
    await retryHistoryEntry(ok.cmds, 7);
    expect(ok.calls).toEqual([["retry_history_entry_transcription", 7]]);

    const ko = mockCommands("error");
    let message = "";
    try {
      await retryHistoryEntry(ko.cmds, 7);
    } catch (e) {
      message = String(e);
    }
    expect(message).toContain("voce non trovata");
  });

  test("elimina chiama delete_history_entry e restituisce l'esito", async () => {
    const ok = mockCommands("ok");
    expect(await deleteHistoryEntry(ok.cmds, 7)).toBe(true);
    expect(ok.calls).toEqual([["delete_history_entry", 7]]);
    const ko = mockCommands("error");
    expect(await deleteHistoryEntry(ko.cmds, 7)).toBe(false);
  });

  test("archivia e riporta nella lista con archive_history_entry(id, flag); il rifiuto si solleva", async () => {
    const ok = mockCommands("ok");
    await archiveHistoryEntry(ok.cmds, 7, true);
    await archiveHistoryEntry(ok.cmds, 7, false);
    expect(ok.calls).toEqual([
      ["archive_history_entry", 7, true],
      ["archive_history_entry", 7, false],
    ]);

    const ko = mockCommands("error");
    let message = "";
    try {
      await archiveHistoryEntry(ko.cmds, 7, true);
    } catch (e) {
      message = String(e);
    }
    expect(message).toContain("voce non trovata");
  });
});
