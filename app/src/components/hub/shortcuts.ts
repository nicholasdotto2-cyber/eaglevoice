/**
 * Scorciatoie dell'hub. Gli id `transcribe` e `cancel` esistono già in Handy;
 * `hands_free`, `paste_last` e `polish` sono gli id che l'interfaccia si aspetta
 * dal backend (da collegare in `settings.rs`, `bindings`). Finché mancano, la
 * riga mostra la combinazione predefinita.
 */
export interface FlowShortcut {
  id: string;
  /** Chiave i18n sotto `hub.shortcuts`. */
  labelKey: string;
  /** Combinazione predefinita, mostrata finché il binding manca. */
  defaultCombo: string;
}

export const FLOW_SHORTCUTS: FlowShortcut[] = [
  { id: "transcribe", labelKey: "pushToTalk", defaultCombo: "Ctrl + Win" },
  { id: "hands_free", labelKey: "handsFree", defaultCombo: "Ctrl + Win + Spazio" },
  { id: "cancel", labelKey: "cancel", defaultCombo: "Esc" },
  { id: "paste_last", labelKey: "pasteLast", defaultCombo: "Shift + Alt + Z" },
  { id: "polish", labelKey: "polish", defaultCombo: "Alt + Win + 1" },
];
