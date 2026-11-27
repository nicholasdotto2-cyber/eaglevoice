/**
 * Suoni della dettatura: anteprima del tema scelto nell'hub.
 * L'accensione e il tema passano da `updateSetting("audio_feedback" | "sound_theme")`
 * dello store, che chiama `change_audio_feedback_setting` e `change_sound_theme_setting`.
 */

export type SoundPreviewKind = "start" | "stop";

/** Anteprima del tema: suono di inizio, poi suono di fine, nell'ordine. */
export const previewSoundTheme = async (
  play: (kind: SoundPreviewKind) => Promise<void>,
): Promise<void> => {
  await play("start");
  await play("stop");
};
