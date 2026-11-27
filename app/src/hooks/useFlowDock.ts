/**
 * Bordo di aggancio della bolla: `flowBarDock()` all'avvio, poi l'evento
 * `flow:dock` (dopo un trascinamento o un cambio di misura). Se il comando non
 * risponde si ripiega su `overlay_position` delle impostazioni.
 */
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { commands, type FlowDockEvent } from "@/bindings";
import { FLOW_EVENTS } from "@/lib/backend";
import { toDockEdge, type DockEdge } from "@/lib/flow";

export interface FlowDock {
  edge: DockEdge;
  offset: number;
}

export const toFlowDock = (value: Partial<FlowDockEvent> | null | undefined): FlowDock => ({
  edge: toDockEdge(value?.edge),
  offset:
    typeof value?.offset === "number" && Number.isFinite(value.offset)
      ? Math.min(1, Math.max(0, value.offset))
      : 0.5,
});

export const useFlowDock = (): FlowDock => {
  const [dock, setDock] = useState<FlowDock>({ edge: "bottom", offset: 0.5 });

  useEffect(() => {
    let alive = true;
    const read = async () => {
      try {
        const res = await commands.flowBarDock();
        if (alive) setDock(toFlowDock(res));
        return;
      } catch (e) {
        console.warn("flow_bar_dock non disponibile, uso overlay_position:", e);
      }
      try {
        const settings = await commands.getAppSettings();
        if (alive && settings.status === "ok") {
          setDock(toFlowDock({ edge: settings.data.overlay_position }));
        }
      } catch {
        // Resta il bordo di default.
      }
    };
    read();
    const unlisten = listen<FlowDockEvent>(FLOW_EVENTS.dock, (event) =>
      setDock(toFlowDock(event.payload)),
    );
    return () => {
      alive = false;
      unlisten.then((fn) => fn());
    };
  }, []);

  return dock;
};
