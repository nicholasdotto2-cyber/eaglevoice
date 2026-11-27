/**
 * Microfoni in ordine di preferenza: sotto la scelta del microfono, una
 * lista con su e giù dei dispositivi presenti più quelli già in
 * `asr.ranked_microphones`. Salva con `update_settings_section("asr", ...)`.
 * Se il campo manca in `AppSettings` vale lista vuota e la riga lo segnala.
 */
import React, { useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronUp } from "lucide-react";
import { useSettings } from "@/hooks/useSettings";
import { useSectionSettings } from "@/hooks/useSectionSettings";
import { mergeRankedMicrophones, moveItem } from "@/lib/microphones";
import { NotConnectedBadge } from "../hub/NotConnected";

export const MicrophoneRanking: React.FC = () => {
  const { t } = useTranslation();
  const { settings, audioDevices, refreshAudioDevices } = useSettings();
  const { value: asr, update, saving } = useSectionSettings("asr");

  useEffect(() => {
    refreshAudioDevices();
  }, [refreshAudioDevices]);

  const storedAsr = (settings as Record<string, unknown> | null)?.asr as
    | { ranked_microphones?: unknown }
    | undefined;
  const fieldPresent = Array.isArray(storedAsr?.ranked_microphones);

  const items = useMemo(
    () =>
      mergeRankedMicrophones(
        asr.ranked_microphones ?? [],
        audioDevices.map((d) => d.name),
      ),
    [asr.ranked_microphones, audioDevices],
  );

  const move = (index: number, delta: number) => {
    const names = moveItem(
      items.map((m) => m.name),
      index,
      delta,
    );
    update({ ranked_microphones: names });
  };

  return (
    <div className="px-4 py-3 space-y-2" data-testid="mic-ranking">
      <div className="flex items-start justify-between gap-3">
        <div>
          <p className="text-sm font-medium">{t("hub.settings.micRanking.title")}</p>
          <p className="text-xs text-mid-gray">
            {t("hub.settings.micRanking.description")}
          </p>
        </div>
        {!fieldPresent && <NotConnectedBadge />}
      </div>
      {items.length === 0 ? (
        <p className="text-sm text-mid-gray">{t("hub.settings.micRanking.empty")}</p>
      ) : (
        <ol className="rounded-xl border border-hairline divide-y divide-hairline">
          {items.map((mic, i) => (
            <li key={mic.name} className="flex items-center gap-3 px-3 py-2">
              <span className="w-5 text-xs text-mid-gray tabular-nums">{i + 1}</span>
              <span
                className={`flex-1 text-sm truncate ${mic.present ? "" : "text-mid-gray"}`}
                title={mic.name}
              >
                {mic.name}
              </span>
              {!mic.present && (
                <span className="text-xs text-mid-gray">
                  {t("hub.settings.micRanking.absent")}
                </span>
              )}
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  className="p-1 rounded-md hover:bg-mid-gray/15 disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer"
                  aria-label={`${t("hub.settings.micRanking.up")}: ${mic.name}`}
                  title={t("hub.settings.micRanking.up")}
                  disabled={i === 0 || saving}
                  onClick={() => move(i, -1)}
                >
                  <ChevronUp className="w-4 h-4" />
                </button>
                <button
                  type="button"
                  className="p-1 rounded-md hover:bg-mid-gray/15 disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer"
                  aria-label={`${t("hub.settings.micRanking.down")}: ${mic.name}`}
                  title={t("hub.settings.micRanking.down")}
                  disabled={i === items.length - 1 || saving}
                  onClick={() => move(i, 1)}
                >
                  <ChevronDown className="w-4 h-4" />
                </button>
              </div>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
};
