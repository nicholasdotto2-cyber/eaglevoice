/**
 * Lettura e scrittura delle sezioni nuove di `AppSettings` (`asr`, `cleanup`)
 * tramite il comando generico `update_settings_section`. Finché il comando o il
 * campo mancano, la sezione vive in memoria con i default e l'interfaccia lo
 * segnala con `notConnected`.
 */
import { useCallback, useMemo, useState } from "react";
import { useSettings } from "./useSettings";
import {
  DEFAULT_ASR_SETTINGS,
  DEFAULT_CLEANUP_SETTINGS,
  updateSettingsSection,
  type AsrSettings,
  type CleanupSettings,
} from "@/lib/backend";

type SectionMap = { asr: AsrSettings; cleanup: CleanupSettings };

const DEFAULTS: SectionMap = {
  asr: DEFAULT_ASR_SETTINGS,
  cleanup: DEFAULT_CLEANUP_SETTINGS,
};

export const useSectionSettings = <K extends keyof SectionMap>(section: K) => {
  const { settings, refreshSettings } = useSettings();
  const [local, setLocal] = useState<SectionMap[K] | null>(null);
  const [notConnected, setNotConnected] = useState(false);
  const [saving, setSaving] = useState(false);

  const stored = (settings as Record<string, unknown> | null)?.[section] as
    | Partial<SectionMap[K]>
    | undefined;
  /** True se `AppSettings` contiene già il campo. */
  const fieldPresent = stored !== undefined;

  const value = useMemo<SectionMap[K]>(
    () => local ?? ({ ...DEFAULTS[section], ...(stored ?? {}) } as SectionMap[K]),
    [local, stored, section],
  );

  const update = useCallback(
    async (patch: Partial<SectionMap[K]>) => {
      const next = { ...value, ...patch } as SectionMap[K];
      setLocal(next);
      setSaving(true);
      try {
        const res = await updateSettingsSection(section, next);
        if (res.ok) {
          setNotConnected(false);
          await refreshSettings();
          setLocal(null);
        } else {
          setNotConnected(res.notConnected || !fieldPresent);
          if (!res.notConnected) console.error(`update_settings_section(${section})`, res.error);
        }
      } finally {
        setSaving(false);
      }
    },
    [value, section, refreshSettings, fieldPresent],
  );

  return {
    value,
    update,
    saving,
    notConnected: notConnected || !fieldPresent,
  };
};
