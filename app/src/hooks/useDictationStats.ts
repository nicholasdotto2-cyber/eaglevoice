/**
 * Parole al minuto vere: ascolta `flow:dictation-stats`, che il backend
 * emette a ogni dettatura incollata, e tiene l'ultima dettatura più la media
 * della sessione (dall'avvio di questa finestra). Lo usano la barra di dettatura e la
 * Home dell'hub, ciascuna nella propria webview.
 */
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { FLOW_EVENTS } from "@/lib/backend";
import {
  addDictationStats,
  EMPTY_SESSION_WPM,
  sessionAverageWpm,
  type DictationStats,
  type SessionWpm,
} from "@/lib/flow";

export interface DictationStatsState {
  session: SessionWpm;
  /** Parole al minuto dell'ultima dettatura, `null` se manca o era sotto il secondo. */
  lastWpm: number | null;
  /** Media della sessione pesata sulla durata, `null` senza dettature valide. */
  averageWpm: number | null;
}

export const useDictationStats = (): DictationStatsState => {
  const [session, setSession] = useState<SessionWpm>(EMPTY_SESSION_WPM);

  useEffect(() => {
    const unlisten = listen<DictationStats>(FLOW_EVENTS.dictationStats, (event) =>
      setSession((prev) => addDictationStats(prev, event.payload)),
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return {
    session,
    lastWpm: session.last?.wpm ?? null,
    averageWpm: sessionAverageWpm(session),
  };
};
