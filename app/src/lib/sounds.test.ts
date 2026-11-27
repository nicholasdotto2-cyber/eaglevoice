import { describe, expect, mock, test } from "bun:test";

// Mock dei bindings: registra ogni comando chiamato dallo store delle impostazioni.
const calls: Array<[string, unknown]> = [];
mock.module("@/bindings", () => ({
  commands: {
    changeAudioFeedbackSetting: async (enabled: boolean) => {
      calls.push(["change_audio_feedback_setting", enabled]);
      return { status: "ok", data: null };
    },
    changeSoundThemeSetting: async (theme: string) => {
      calls.push(["change_sound_theme_setting", theme]);
      return { status: "ok", data: null };
    },
    playTestSound: async (soundType: string) => {
      calls.push(["play_test_sound", soundType]);
    },
  },
}));

const { useSettingsStore } = await import("@/stores/settingsStore");
const { previewSoundTheme } = await import("./sounds");

describe("suoni dall'hub", () => {
  test("accendere i suoni e scegliere il tema chiamano i comandi del backend", async () => {
    calls.length = 0;
    const store = useSettingsStore.getState();
    await store.updateSetting("audio_feedback", true);
    await store.updateSetting("sound_theme", "pop");
    expect(calls).toEqual([
      ["change_audio_feedback_setting", true],
      ["change_sound_theme_setting", "pop"],
    ]);
  });

  test("l'anteprima suona inizio e poi fine con play_test_sound", async () => {
    calls.length = 0;
    await previewSoundTheme(useSettingsStore.getState().playTestSound);
    expect(calls).toEqual([
      ["play_test_sound", "start"],
      ["play_test_sound", "stop"],
    ]);
  });
});
