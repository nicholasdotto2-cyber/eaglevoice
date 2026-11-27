import React from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Button } from "../ui/Button";
import { NotConnectedBadge } from "./NotConnected";
import type { EngineId, EngineView } from "@/hooks/useEngineState";
import type { BackendResult } from "@/lib/backend";

const DOT: Record<string, string> = {
  ready: "bg-green-500",
  starting: "bg-amber-400 animate-pulse",
  stopped: "bg-mid-gray",
  error: "bg-red-500",
};

interface EngineCardProps {
  engine: EngineId;
  view: EngineView;
  restarting: boolean;
  onRestart?: (engine: EngineId) => Promise<BackendResult<null>>;
  compact?: boolean;
}

export const EngineCard: React.FC<EngineCardProps> = ({
  engine,
  view,
  restarting,
  onRestart,
  compact = false,
}) => {
  const { t } = useTranslation();
  const s = view.state;

  const handleRestart = async () => {
    if (!onRestart) return;
    const res = await onRestart(engine);
    if (!res.ok) {
      toast.error(
        res.notConnected ? t("hub.notConnected") : t("engines.restartError"),
        { description: res.notConnected ? undefined : res.error },
      );
    }
  };

  return (
    <div className="flex items-start gap-3 px-4 py-3">
      <span
        className={`mt-1.5 w-2.5 h-2.5 rounded-full shrink-0 ${
          s ? DOT[s.state] : "bg-mid-gray/40"
        }`}
      />
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <p className="text-sm font-medium">{t(`engines.${engine}.name`)}</p>
          {view.notConnected && <NotConnectedBadge />}
        </div>
        <p className="text-xs text-mid-gray">
          {s
            ? t(`engines.state.${s.state}`)
            : view.notConnected
              ? t("engines.state.unknown")
              : t("engines.state.waiting")}
          {s?.model ? ` · ${s.model}` : ""}
          {!compact && s?.vram_mb ? ` · ${t("engines.vram", { mb: s.vram_mb })}` : ""}
        </p>
        {!compact && s?.detail && (
          <p className="text-xs text-mid-gray mt-0.5 break-words select-text">
            {s.detail}
          </p>
        )}
      </div>
      {onRestart && (
        <Button
          variant="secondary"
          size="sm"
          className="flex items-center gap-1.5 shrink-0"
          onClick={handleRestart}
          disabled={restarting}
        >
          <RefreshCw className={`w-3.5 h-3.5 ${restarting ? "animate-spin" : ""}`} />
          {t("engines.restart")}
        </Button>
      )}
    </div>
  );
};
