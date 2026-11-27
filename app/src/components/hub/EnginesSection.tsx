/**
 * Motori: stato e riavvio di whisper.cpp e llama-server, percorsi in sola
 * lettura, poi i modelli di Handy (motore di riserva in-process) e, se attiva,
 * la post-elaborazione di Handy.
 */
import React from "react";
import { useTranslation } from "react-i18next";
import { useEngineState } from "@/hooks/useEngineState";
import { useSectionSettings } from "@/hooks/useSectionSettings";
import { useSettings } from "@/hooks/useSettings";
import { SettingsGroup } from "../ui/SettingsGroup";
import { ModelsSettings } from "../settings/models/ModelsSettings";
import { PostProcessingSettings } from "../settings/post-processing/PostProcessingSettings";
import { EngineCard } from "./EngineCard";

const ReadOnlyPath: React.FC<{ label: string; value: string }> = ({
  label,
  value,
}) => {
  const { t } = useTranslation();
  return (
    <div className="px-4 py-2">
      <p className="text-xs text-mid-gray mb-1">{label}</p>
      <div className="px-2 py-1.5 bg-mid-gray/10 border border-mid-gray/30 rounded-md text-xs font-mono break-all select-text cursor-text">
        {value || t("engines.pathUnset")}
      </div>
    </div>
  );
};

export const EnginesSection: React.FC = () => {
  const { t } = useTranslation();
  const engines = useEngineState();
  const asr = useSectionSettings("asr");
  const cleanup = useSectionSettings("cleanup");
  const { settings } = useSettings();

  return (
    <div className="max-w-3xl w-full mx-auto space-y-8">
      <SettingsGroup title={t("engines.title")} description={t("engines.description")}>
        <EngineCard
          engine="asr"
          view={engines.asr}
          restarting={engines.restarting === "asr"}
          onRestart={engines.restart}
        />
        <ReadOnlyPath label={t("engines.asr.bin")} value={asr.value.whisper_bin_dir} />
        <ReadOnlyPath label={t("engines.asr.model")} value={asr.value.whisper_model_path} />
        <EngineCard
          engine="cleanup"
          view={engines.cleanup}
          restarting={engines.restarting === "cleanup"}
          onRestart={engines.restart}
        />
        <ReadOnlyPath label={t("engines.cleanup.bin")} value={cleanup.value.llama_bin_dir} />
        <ReadOnlyPath label={t("engines.cleanup.model")} value={cleanup.value.llama_model_path} />
      </SettingsGroup>

      <div className="space-y-2">
        <div className="px-4">
          <h2 className="text-[13px] font-semibold text-text">
            {t("engines.fallbackTitle")}
          </h2>
          <p className="text-xs text-mid-gray mt-1">{t("engines.fallbackDescription")}</p>
        </div>
        <ModelsSettings />
      </div>

      {settings?.post_process_enabled && <PostProcessingSettings />}
    </div>
  );
};
