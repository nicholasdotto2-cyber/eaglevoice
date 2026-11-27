import { describe, expect, test } from "bun:test";
import i18next from "i18next";
import {
  addDictationStats,
  classifyNotices,
  compactShortcut,
  EMPTY_SESSION_WPM,
  formatElapsed,
  pastePreview,
  sessionAverageWpm,
  isSilentFrame,
  MIC_SILENT_AFTER_MS,
  toDockEdge,
  normalizeFlowState,
  isTransformFlow,
  type NoticeInputs,
} from "./flow";
import type { EngineState } from "./backend";

const ready: EngineState = { state: "ready", detail: null, model: "m", vram_mb: 100 };
const base: NoticeInputs = {
  recordingError: null,
  asr: ready,
  cleanup: ready,
  qualityMode: false,
  silentForMs: 0,
};
const kinds = (input: Partial<NoticeInputs>) =>
  classifyNotices({ ...base, ...input }).map((n) => n.kind);

describe("parole al minuto da flow:dictation-stats", () => {
  test("ultima dettatura e media della sessione pesata sulla durata", () => {
    let s = addDictationStats(EMPTY_SESSION_WPM, { words: 30, audio_secs: 15, wpm: 120 });
    s = addDictationStats(s, { words: 90, audio_secs: 45, wpm: 120 });
    s = addDictationStats(s, { words: 20, audio_secs: 20, wpm: 60 });
    expect(s.last?.wpm).toBe(60);
    expect(s.count).toBe(3);
    // 140 parole in 80 secondi: 105 al minuto, non la media semplice (100).
    expect(sessionAverageWpm(s)).toBe(105);
  });
  test("una dettatura senza wpm aggiorna l'ultima ma non la media", () => {
    expect(sessionAverageWpm(EMPTY_SESSION_WPM)).toBeNull();
    const s = addDictationStats(EMPTY_SESSION_WPM, { words: 1, audio_secs: 0.4, wpm: null });
    expect(s.last?.words).toBe(1);
    expect(s.count).toBe(0);
    expect(sessionAverageWpm(s)).toBeNull();
  });
  test("timer m:ss", () => {
    expect(formatElapsed(0)).toBe("0:00");
    expect(formatElapsed(65_400)).toBe("1:05");
  });
});

describe("barra a riposo", () => {
  test("scorciatoia corta per la pillola", () => {
    expect(compactShortcut("Left Ctrl + Left Meta")).toBe("Ctrl + Win");
    expect(compactShortcut("Shift + Alt + Z")).toBe("Shift + Alt + Z");
  });
});

describe("incolla non riuscito (flow:paste-failed)", () => {
  test("la notifica mostra l'inizio del testo su una riga", () => {
    expect(pastePreview("  Parola uno,\n parola due  parola tre ")).toBe(
      "Parola uno, parola due parola tre",
    );
    const long = "parola ".repeat(20);
    const preview = pastePreview(long, 20);
    expect(preview.endsWith("...")).toBe(true);
    expect(preview.length <= 23).toBe(true);
  });
});

describe("notifiche dagli eventi", () => {
  test("tutto pronto: nessuna notifica", () => {
    expect(kinds({})).toEqual([]);
  });
  test("eventi assenti (backend non collegato): nessuna notifica", () => {
    expect(kinds({ asr: null, cleanup: null })).toEqual([]);
  });
  test("microfono assente, bloccato, altro errore", () => {
    expect(kinds({ recordingError: { error_type: "no_input_device" } })).toEqual(["mic_missing"]);
    expect(kinds({ recordingError: { error_type: "microphone_permission_denied" } })).toEqual(["mic_blocked"]);
    expect(kinds({ recordingError: { error_type: "unknown", detail: "x" } })).toEqual(["mic_error"]);
  });
  test("microfono muto dopo la soglia di silenzio", () => {
    expect(kinds({ silentForMs: MIC_SILENT_AFTER_MS - 1 })).toEqual([]);
    expect(kinds({ silentForMs: MIC_SILENT_AFTER_MS })).toEqual(["mic_silent"]);
  });
  test("motore ASR in avvio o non pronto, verso la sezione Motori", () => {
    const starting = classifyNotices({ ...base, asr: { ...ready, state: "starting" } });
    expect(starting.map((n) => n.kind)).toEqual(["asr_starting"]);
    expect(starting[0].target).toBe("models");
    expect(kinds({ asr: { ...ready, state: "error", detail: "crash" } })).toEqual(["asr_not_ready"]);
    expect(kinds({ asr: { ...ready, state: "stopped" } })).toEqual(["asr_not_ready"]);
  });
  test("rete assente solo in modalità qualità", () => {
    const offline: EngineState = { ...ready, state: "error", detail: "network: dns" };
    expect(kinds({ cleanup: offline, qualityMode: true })).toEqual(["network_offline"]);
    expect(kinds({ cleanup: offline, qualityMode: false })).toEqual(["cleanup_not_ready"]);
    expect(kinds({ cleanup: { ...ready, state: "starting" } })).toEqual(["cleanup_starting"]);
  });
  test("più problemi insieme, in ordine di gravità", () => {
    expect(
      kinds({
        recordingError: { error_type: "no_input_device" },
        asr: { ...ready, state: "error" },
        cleanup: { ...ready, state: "stopped" },
      }),
    ).toEqual(["mic_missing", "asr_not_ready", "cleanup_not_ready"]);
  });
  test("avviso di durata: primo, senza sezione dell'hub, solo se l'evento è arrivato", () => {
    expect(kinds({ durationWarningSecs: null })).toEqual([]);
    const notices = classifyNotices({
      ...base,
      durationWarningSecs: 1140,
      cleanup: { ...ready, state: "starting" },
    });
    expect(notices.map((n) => n.kind)).toEqual(["duration_warning", "cleanup_starting"]);
    expect(notices[0].target).toBeNull();
  });
  test("avviso di durata: il testo usa il tetto delle impostazioni, non 20", async () => {
    const [notice] = classifyNotices({
      ...base,
      durationWarningSecs: 60,
      maxRecordingMinutes: 2,
    });
    expect(notice.params).toEqual({ minutes: 2 });
    const it = (await import("../i18n/locales/it/translation.json")).default;
    const en = (await import("../i18n/locales/en/translation.json")).default;
    const i18n = i18next.createInstance();
    await i18n.init({
      lng: "it",
      resources: { it: { translation: it }, en: { translation: en } },
    });
    expect(i18n.t(`notices.${notice.kind}`, notice.params)).toBe(
      "Manca un minuto: la registrazione si ferma a 2 minuti",
    );
    expect(i18n.t(`notices.${notice.kind}`, { ...notice.params, lng: "en" })).toBe(
      "One minute left: recording stops at 2 minutes",
    );
    // Impostazioni non lette: i minuti dai secondi dell'evento (un minuto prima del tetto).
    const [fallback] = classifyNotices({ ...base, durationWarningSecs: 240 });
    expect(fallback.params).toEqual({ minutes: 5 });
  });
  test("frame silenzioso", () => {
    expect(isSilentFrame([0, 0.01, 0.005])).toBe(true);
    expect(isSilentFrame([0, 0.3])).toBe(false);
    expect(isSilentFrame([])).toBe(true);
  });
  test("bordo di aggancio", () => {
    expect(toDockEdge("right")).toBe("right");
    expect(toDockEdge("none")).toBe("bottom");
    expect(toDockEdge(undefined)).toBe("bottom");
  });
});

describe("flow:state dal backend del 18/11", () => {
  test("tocco singolo scartato: a riposo, niente Annullato; un annullo vero resta", () => {
    expect(normalizeFlowState({ state: "cancelled", detail: "tap" }).state).toBe("idle");
    expect(normalizeFlowState({ state: "cancelled", detail: null }).state).toBe("cancelled");
  });
  test("stati del Transform: li mostra flow:transform", () => {
    expect(isTransformFlow("transform")).toBe(true);
    expect(isTransformFlow("transform_unchanged")).toBe(true);
    expect(isTransformFlow("tap")).toBe(false);
    expect(isTransformFlow(null)).toBe(false);
  });
});
