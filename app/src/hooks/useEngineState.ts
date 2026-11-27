/**
 * Stato dei due motori esterni per l'hub: whisper.cpp (asr) e llama-server
 * (cleanup). Legge lo stato iniziale con `asr_get_state` e `cleanup_get_state`,
 * poi segue gli eventi `asr:engine-state` e `cleanup:engine-state`.
 */
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  asrGetState,
  asrRestartEngine,
  cleanupGetState,
  cleanupQualityKeyPresent,
  cleanupRestartEngine,
  FLOW_EVENTS,
  type BackendResult,
  type EngineState,
} from "@/lib/backend";

export type EngineId = "asr" | "cleanup";

export interface EngineView {
  /** `null` finché non arriva né la risposta del comando né un evento. */
  state: EngineState | null;
  /** True se il comando Rust non esiste ancora. */
  notConnected: boolean;
  error: string | null;
}

const emptyView: EngineView = { state: null, notConnected: false, error: null };

const toView = (res: BackendResult<EngineState>): EngineView =>
  res.ok
    ? { state: res.data, notConnected: false, error: null }
    : { state: null, notConnected: res.notConnected, error: res.error };

export const useEngineState = () => {
  const [asr, setAsr] = useState<EngineView>(emptyView);
  const [cleanup, setCleanup] = useState<EngineView>(emptyView);
  /** `null` = comando non collegato o non ancora letto. */
  const [qualityKeyPresent, setQualityKeyPresent] = useState<boolean | null>(
    null,
  );
  const [restarting, setRestarting] = useState<EngineId | null>(null);

  const refresh = useCallback(async () => {
    const [a, c, k] = await Promise.all([
      asrGetState(),
      cleanupGetState(),
      cleanupQualityKeyPresent(),
    ]);
    setAsr(toView(a));
    setCleanup(toView(c));
    setQualityKeyPresent(k.ok ? k.data : null);
  }, []);

  useEffect(() => {
    refresh();
    const unlisteners = [
      listen<EngineState>(FLOW_EVENTS.asrEngineState, (e) =>
        setAsr({ state: e.payload, notConnected: false, error: null }),
      ),
      listen<EngineState>(FLOW_EVENTS.cleanupEngineState, (e) =>
        setCleanup({ state: e.payload, notConnected: false, error: null }),
      ),
    ];
    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, [refresh]);

  const restart = useCallback(
    async (engine: EngineId): Promise<BackendResult<null>> => {
      setRestarting(engine);
      try {
        const res =
          engine === "asr"
            ? await asrRestartEngine()
            : await cleanupRestartEngine();
        await refresh();
        return res;
      } finally {
        setRestarting(null);
      }
    },
    [refresh],
  );

  return { asr, cleanup, qualityKeyPresent, restarting, restart, refresh };
};
