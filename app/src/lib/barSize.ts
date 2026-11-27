/**
 * Misura della finestra della bolla quando c'è un avviso. A riposo la finestra
 * è 52x52 (o nascosta) e durante la dettatura 300x72: un avviso non ci sta.
 * Quando ne arriva uno, la bolla chiede `flow_expand_bar` (360x190, anche
 * durante la dettatura: flow.rs allarga la finestra senza cambiare stato);
 * quando l'ultimo si chiude, `flow_rest_bar`.
 *
 * Logica pura con l'adapter iniettato, così si prova con `bun test`.
 */

export interface BarSizeAdapter {
  expand: () => unknown;
  rest: () => unknown;
}

export interface BarSizeInput {
  /** Avvisi aperti, compreso il testo non incollato. */
  openNotices: number;
}

export type BarSizeAction = "expand" | "rest" | null;

export interface BarSizer {
  /** Il backend ha mostrato da solo la finestra grande (`show-overlay` "notice"). */
  markExpanded: () => void;
  /**
   * Il backend ha rimesso la finestra alla misura dello stato o l'ha nascosta
   * (`show-overlay` "idle" o "recording", `hide-overlay`): l'avviso, se c'è
   * ancora, va richiesto di nuovo.
   */
  markRested: () => void;
  update: (input: BarSizeInput) => BarSizeAction;
}

export const createBarSizer = (adapter: BarSizeAdapter): BarSizer => {
  let expanded = false;
  return {
    markExpanded: () => {
      expanded = true;
    },
    markRested: () => {
      expanded = false;
    },
    update: ({ openNotices }) => {
      if (openNotices > 0 && !expanded) {
        expanded = true;
        adapter.expand();
        return "expand";
      }
      if (openNotices === 0 && expanded) {
        expanded = false;
        adapter.rest();
        return "rest";
      }
      return null;
    },
  };
};

/** Quanto resta visibile "Copiato" prima che l'avviso si chiuda da solo. */
export const COPIED_CLOSE_MS = 1500;
