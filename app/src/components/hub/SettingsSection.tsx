/**
 * Pezzi delle Impostazioni: scorciatoie, microfono,
 * suoni, lingue, barra, tema, modalità qualità. La finestra modale delle
 * Impostazioni (SettingsModal.tsx) li compone per gruppo.
 * Riusa i componenti di Handy dove esistono.
 */
import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { type } from "@tauri-apps/plugin-os";
import type { OverlayPosition, Theme } from "@/bindings";
import { setFlowBarPasteWpm, setFlowBarVisible } from "@/lib/backend";
import { applyTheme } from "@/lib/utils/theme";
import { useSettings } from "@/hooks/useSettings";
import { useSectionSettings } from "@/hooks/useSectionSettings";
import { useEngineState } from "@/hooks/useEngineState";
import { DOCK_EDGES, toDockEdge } from "@/lib/flow";
import { SettingsGroup } from "../ui/SettingsGroup";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { Dropdown } from "../ui/Dropdown";
import { ShortcutInput } from "../settings/ShortcutInput";
import { ShortcutActivationSetting } from "../settings/ShortcutActivation";
import { MicrophoneSelector } from "../settings/MicrophoneSelector";
import { MicrophoneRanking } from "../settings/MicrophoneRanking";
import { ChannelSelector } from "../settings/ChannelSelector";
import { MuteWhileRecording } from "../settings/MuteWhileRecording";
import { AudioFeedback } from "../settings/AudioFeedback";
import { SoundPicker } from "../settings/SoundPicker";
import { OutputDeviceSelector } from "../settings/OutputDeviceSelector";
import { VolumeSlider } from "../settings/VolumeSlider";
import { AutostartToggle } from "../settings/AutostartToggle";
import { NotConnectedBadge } from "./NotConnected";
import { FLOW_SHORTCUTS } from "./shortcuts";

/** Lingue proposte nella selezione multipla. Di default sono attive it ed en. */
const LANGUAGE_CHOICES = ["it", "en", "es", "fr", "de"];

export const LanguagesSetting: React.FC = () => {
  const { t } = useTranslation();
  const { value: asr, update, notConnected } = useSectionSettings("asr");

  const toggle = (code: string) => {
    const has = asr.languages.includes(code);
    if (has && asr.languages.length === 1) return; // almeno una lingua
    const languages = has
      ? asr.languages.filter((l) => l !== code)
      : [...asr.languages, code];
    const primary_language = languages.includes(asr.primary_language)
      ? asr.primary_language
      : languages[0];
    update({ languages, primary_language });
  };

  return (
    <>
      <SettingContainer
        title={t("hub.settings.languages.title")}
        description={t("hub.settings.languages.description")}
        grouped
      >
        <div className="flex flex-wrap items-center gap-1 justify-end">
          {notConnected && <NotConnectedBadge />}
          {LANGUAGE_CHOICES.map((code) => {
            const on = asr.languages.includes(code);
            return (
              <button
                key={code}
                type="button"
                onClick={() => toggle(code)}
                className={`px-2.5 py-1 rounded-full text-xs border cursor-pointer ${
                  on
                    ? "bg-logo-primary text-on-primary border-logo-primary"
                    : "border-mid-gray/30 hover:bg-mid-gray/15"
                }`}
              >
                {t(`hub.settings.languages.names.${code}`)}
              </button>
            );
          })}
        </div>
      </SettingContainer>
      <SettingContainer
        title={t("hub.settings.languages.primary")}
        description={t("hub.settings.languages.primaryDescription")}
        grouped
      >
        <Dropdown
          options={asr.languages.map((code) => ({
            value: code,
            label: t(`hub.settings.languages.names.${code}`),
          }))}
          selectedValue={asr.primary_language}
          onSelect={(v) => update({ primary_language: v })}
        />
      </SettingContainer>
      <ToggleSwitch
        grouped
        checked={asr.language_detection}
        onChange={(v) => update({ language_detection: v })}
        label={t("hub.settings.languages.detect")}
        description={t("hub.settings.languages.detectDescription")}
        descriptionMode="inline"
      />
    </>
  );
};

export const DockEdgeSetting: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const edge = toDockEdge(getSetting("overlay_position"));
  return (
    <SettingContainer
      title={t("hub.settings.dock.title")}
      description={t("hub.settings.dock.description")}
      grouped
    >
      <Dropdown
        options={DOCK_EDGES.map((e) => ({
          value: e,
          label:
            e === "left" || e === "right"
              ? t("hub.settings.dock.pending", {
                  edge: t(`hub.settings.dock.options.${e}`),
                })
              : t(`hub.settings.dock.options.${e}`),
        }))}
        selectedValue={edge}
        // left e right passano dal comando di Handy: Rust li accetta dopo l'estensione di OverlayPosition.
        onSelect={(v) => updateSetting("overlay_position", v as OverlayPosition)}
        disabled={isUpdating("overlay_position")}
      />
    </SettingContainer>
  );
};

/** "Mostra sempre la barra di dettatura", legato a `flow_bar_always_visible` e a `set_flow_bar_visible`. */
export const FlowBarVisibleSetting: React.FC = () => {
  const { t } = useTranslation();
  const { settings, refreshSettings } = useSettings();
  const [local, setLocal] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [notConnected, setNotConnected] = useState(false);
  const stored = (settings as Record<string, unknown> | null)
    ?.flow_bar_always_visible;
  // Predefinito lato Rust: true.
  const checked = local ?? (typeof stored === "boolean" ? stored : true);

  const onChange = async (visible: boolean) => {
    setLocal(visible);
    setBusy(true);
    try {
      const res = await setFlowBarVisible(visible);
      if (res.ok) {
        setNotConnected(false);
        await refreshSettings();
        setLocal(null);
      } else {
        setNotConnected(res.notConnected);
        if (!res.notConnected) {
          console.error("set_flow_bar_visible:", res.error);
          setLocal(null);
        }
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <ToggleSwitch
        grouped
        checked={checked}
        isUpdating={busy}
        onChange={onChange}
        label={t("hub.settings.flowBar.title")}
        description={t("hub.settings.flowBar.description")}
        descriptionMode="inline"
      />
      {notConnected && (
        <div className="px-4 pb-2">
          <NotConnectedBadge />
        </div>
      )}
    </>
  );
};

export const QualityModeSetting: React.FC = () => {
  const { t } = useTranslation();
  const { value: cleanup, update, notConnected } = useSectionSettings("cleanup");
  const { qualityKeyPresent } = useEngineState();

  return (
    <>
      <ToggleSwitch
        grouped
        checked={cleanup.quality_mode}
        onChange={(v) => update({ quality_mode: v })}
        label={t("hub.settings.quality.title")}
        description={t("hub.settings.quality.description", {
          model: cleanup.quality_model,
        })}
      />
      <div className="flex items-center justify-between px-4 py-2">
        <p className="text-sm">{t("hub.settings.quality.keyLabel")}</p>
        <div className="flex items-center gap-2">
          {(notConnected || qualityKeyPresent === null) && <NotConnectedBadge />}
          <span
            className={`text-sm font-medium ${
              qualityKeyPresent ? "text-success" : "text-mid-gray"
            }`}
          >
            {qualityKeyPresent === null
              ? t("hub.settings.quality.keyUnknown")
              : qualityKeyPresent
                ? t("hub.settings.quality.keyPresent")
                : t("hub.settings.quality.keyMissing")}
          </span>
        </div>
      </div>
      {qualityKeyPresent === false && (
        <p className="px-4 pb-3 text-xs text-mid-gray">
          {t("hub.settings.quality.keyHint")}
        </p>
      )}
    </>
  );
};

/** Tema dell'hub: Cobalto (di sempre) o Chiaro. "system" vale Cobalto. */
export const ThemeSetting: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const current: Theme = getSetting("theme") === "light" ? "light" : "dark";
  return (
    <SettingContainer
      title={t("hub.settings.theme.title")}
      description={t("hub.settings.theme.description")}
      grouped
    >
      <Dropdown
        options={(["dark", "light"] as Theme[]).map((v) => ({
          value: v,
          label: t(`hub.settings.theme.options.${v}`),
        }))}
        selectedValue={current}
        onSelect={(v) => {
          applyTheme(v as Theme);
          updateSetting("theme", v as Theme);
        }}
        disabled={isUpdating("theme")}
      />
    </SettingContainer>
  );
};

/** "p/m" accanto alla spunta all'incolla, predefinito spento. */
export const PasteWpmSetting: React.FC = () => {
  const { t } = useTranslation();
  const { settings, refreshSettings } = useSettings();
  const [local, setLocal] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [notConnected, setNotConnected] = useState(false);
  const checked = local ?? settings?.flow_bar_paste_wpm ?? false;

  // `flow_bar_paste_wpm` di AppSettings, scritto con set_flow_bar_paste_wpm.
  const onChange = async (on: boolean) => {
    setLocal(on);
    setBusy(true);
    try {
      const res = await setFlowBarPasteWpm(on);
      setNotConnected(!res.ok && res.notConnected);
      if (!res.ok && !res.notConnected) console.error("set_flow_bar_paste_wpm:", res.error);
      if (res.ok) await refreshSettings();
    } finally {
      setLocal(null);
      setBusy(false);
    }
  };

  return (
    <>
      <ToggleSwitch
        grouped
        checked={checked}
        onChange={onChange}
        isUpdating={busy}
        label={t("hub.settings.pasteWpm.title")}
        description={t("hub.settings.pasteWpm.description")}
        descriptionMode="inline"
      />
      {notConnected && (
        <div className="px-4 pb-2">
          <NotConnectedBadge />
        </div>
      )}
    </>
  );
};

/** Scorciatoie: quelle di Handy più quelle di eagleVoice, con l'attivazione. */
export const ShortcutsGroup: React.FC = () => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const isLinux = type() === "linux";
  return (
    <SettingsGroup title={t("hub.settings.shortcuts")}>
      {FLOW_SHORTCUTS.map((s) => {
        if (s.id === "cancel" && isLinux) return null;
        if (settings?.bindings?.[s.id]) {
          return <ShortcutInput key={s.id} shortcutId={s.id} grouped />;
        }
        return (
          <SettingContainer
            key={s.id}
            title={t(`hub.shortcuts.${s.labelKey}`)}
            description={t("hub.shortcuts.pendingDescription")}
            grouped
          >
            <div className="flex items-center gap-2">
              <NotConnectedBadge />
              <kbd className="px-2 py-1 text-xs font-semibold rounded-md bg-mid-gray/10 border border-mid-gray/30">
                {s.defaultCombo}
              </kbd>
            </div>
          </SettingContainer>
        );
      })}
      <ShortcutActivationSetting descriptionMode="tooltip" grouped />
    </SettingsGroup>
  );
};

export const MicrophoneGroup: React.FC = () => {
  const { t } = useTranslation();
  return (
    <SettingsGroup title={t("hub.settings.microphone")}>
      <MicrophoneSelector descriptionMode="tooltip" grouped />
      <MicrophoneRanking />
      <ChannelSelector descriptionMode="tooltip" grouped />
      <MuteWhileRecording descriptionMode="tooltip" grouped />
    </SettingsGroup>
  );
};

export const SoundsGroup: React.FC = () => {
  const { t } = useTranslation();
  const { audioFeedbackEnabled } = useSettings();
  return (
    <SettingsGroup title={t("hub.settings.sounds")}>
      <AudioFeedback descriptionMode="tooltip" grouped />
      <SoundPicker
        label={t("hub.settings.soundTheme.label")}
        description={t("hub.settings.soundTheme.description")}
        previewLabel={t("hub.settings.soundTheme.preview")}
      />
      <OutputDeviceSelector
        descriptionMode="tooltip"
        grouped
        disabled={!audioFeedbackEnabled}
      />
      <VolumeSlider disabled={!audioFeedbackEnabled} />
    </SettingsGroup>
  );
};

