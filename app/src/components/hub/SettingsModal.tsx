/**
 * Impostazioni in finestra modale: circa 960x640,
 * raggio 12, barra laterale di 192 px con i gruppi Generali, Sistema, Motori,
 * Informazioni. Raccoglie tutto ciò che prima stava nelle pagine Impostazioni,
 * Avanzate, Motori e Info dell'hub, senza togliere nulla.
 */
import React, { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { useSettings } from "@/hooks/useSettings";
import { SettingsGroup } from "../ui/SettingsGroup";
import { AutostartToggle } from "../settings/AutostartToggle";
import { StartHidden } from "../settings/StartHidden";
import { ShowTrayIcon } from "../settings/ShowTrayIcon";
import { ShowOverlay } from "../settings/ShowOverlay";
import { ModelUnloadTimeoutSetting } from "../settings/ModelUnloadTimeout";
import { ExperimentalToggle } from "../settings/ExperimentalToggle";
import { PasteMethodSetting } from "../settings/PasteMethod";
import { TypingToolSetting } from "../settings/TypingTool";
import { ClipboardHandlingSetting } from "../settings/ClipboardHandling";
import { AutoSubmit } from "../settings/AutoSubmit";
import { AppendTrailingSpace } from "../settings/AppendTrailingSpace";
import { VoiceActivityDetection } from "../settings/VoiceActivityDetection";
import { LongFormChunking } from "../settings/LongFormChunking";
import { FillerWordRemoval } from "../settings/FillerWordRemoval";
import { ChineseScriptSetting } from "../settings/ChineseScript";
import { CustomWords } from "../settings/CustomWords";
import { HistoryLimit } from "../settings/HistoryLimit";
import { RecordingRetentionPeriodSelector } from "../settings/RecordingRetentionPeriod";
import { PostProcessingToggle } from "../settings/PostProcessingToggle";
import { KeyboardImplementationSelector } from "../settings/debug/KeyboardImplementationSelector";
import { AccelerationSelector } from "../settings/AccelerationSelector";
import { LazyStreamClose } from "../settings/LazyStreamClose";
import { VadBackendSelector } from "../settings/VadBackendSelector";
import { AboutSettings } from "../settings/about/AboutSettings";
import { DebugSettings } from "../settings/debug/DebugSettings";
import { EnginesSection } from "./EnginesSection";
import {
  DockEdgeSetting,
  FlowBarVisibleSetting,
  LanguagesSetting,
  MicrophoneGroup,
  PasteWpmSetting,
  QualityModeSetting,
  ShortcutsGroup,
  SoundsGroup,
  ThemeSetting,
} from "./SettingsSection";

export type SettingsTab =
  | "shortcuts"
  | "microphone"
  | "languages"
  | "app"
  | "bar"
  | "sounds"
  | "paste"
  | "data"
  | "experimental"
  | "engines"
  | "transcription"
  | "quality"
  | "about"
  | "debug";

interface TabGroup {
  key: string;
  tabs: SettingsTab[];
}

export const SETTINGS_GROUPS: TabGroup[] = [
  { key: "general", tabs: ["shortcuts", "microphone", "languages"] },
  { key: "system", tabs: ["app", "bar", "sounds", "paste", "data", "experimental"] },
  { key: "engines", tabs: ["engines", "transcription", "quality"] },
  { key: "info", tabs: ["about", "debug"] },
];

/** Sezione dell'hub di prima (`hub:navigate`) che oggi sta nella finestra delle Impostazioni. */
export const tabForSection = (section: string): SettingsTab | null => {
  switch (section) {
    case "settings":
      return "microphone";
    case "models":
      return "engines";
    case "advanced":
      return "app";
    case "about":
      return "about";
    case "debug":
      return "debug";
    default:
      return null;
  }
};

const TabContent: React.FC<{ tab: SettingsTab }> = ({ tab }) => {
  const { t } = useTranslation();
  switch (tab) {
    case "shortcuts":
      return <ShortcutsGroup />;
    case "microphone":
      return <MicrophoneGroup />;
    case "languages":
      return (
        <SettingsGroup title={t("hub.settings.languages.group")}>
          <LanguagesSetting />
        </SettingsGroup>
      );
    case "app":
      return (
        <SettingsGroup title={t("hub.settings.app")}>
          <AutostartToggle descriptionMode="tooltip" grouped />
          <StartHidden descriptionMode="tooltip" grouped />
          <ShowTrayIcon descriptionMode="tooltip" grouped />
          <ThemeSetting />
          <ModelUnloadTimeoutSetting descriptionMode="tooltip" grouped />
          <ExperimentalToggle descriptionMode="tooltip" grouped />
        </SettingsGroup>
      );
    case "bar":
      return (
        <SettingsGroup title={t("hubLayout.settings.tabs.bar")}>
          <FlowBarVisibleSetting />
          <DockEdgeSetting />
          <PasteWpmSetting />
          <ShowOverlay descriptionMode="tooltip" grouped />
        </SettingsGroup>
      );
    case "sounds":
      return <SoundsGroup />;
    case "paste":
      return (
        <SettingsGroup title={t("settings.advanced.groups.output")}>
          <PasteMethodSetting descriptionMode="tooltip" grouped />
          <TypingToolSetting descriptionMode="tooltip" grouped />
          <ClipboardHandlingSetting descriptionMode="tooltip" grouped />
          <AutoSubmit descriptionMode="tooltip" grouped />
          <AppendTrailingSpace descriptionMode="tooltip" grouped />
        </SettingsGroup>
      );
    case "data":
      return (
        <SettingsGroup title={t("settings.advanced.groups.history")}>
          <HistoryLimit descriptionMode="tooltip" grouped />
          <RecordingRetentionPeriodSelector descriptionMode="tooltip" grouped />
        </SettingsGroup>
      );
    case "experimental":
      return (
        <SettingsGroup title={t("settings.advanced.groups.experimental")}>
          <PostProcessingToggle descriptionMode="tooltip" grouped />
          <KeyboardImplementationSelector descriptionMode="tooltip" grouped />
          <AccelerationSelector descriptionMode="tooltip" grouped />
          <LazyStreamClose descriptionMode="tooltip" grouped />
          <VadBackendSelector descriptionMode="tooltip" grouped />
        </SettingsGroup>
      );
    case "engines":
      return <EnginesSection />;
    case "transcription":
      return (
        <SettingsGroup title={t("settings.advanced.groups.transcription")}>
          <VoiceActivityDetection descriptionMode="tooltip" grouped />
          <LongFormChunking descriptionMode="tooltip" grouped />
          <FillerWordRemoval descriptionMode="tooltip" grouped />
          <ChineseScriptSetting descriptionMode="tooltip" grouped />
          <CustomWords descriptionMode="tooltip" grouped />
        </SettingsGroup>
      );
    case "quality":
      return (
        <SettingsGroup title={t("hub.settings.quality.group")}>
          <QualityModeSetting />
        </SettingsGroup>
      );
    case "about":
      return <AboutSettings />;
    case "debug":
      return <DebugSettings />;
  }
};

interface SettingsModalProps {
  open: boolean;
  tab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
  onClose: () => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  open,
  tab,
  onTabChange,
  onClose,
}) => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const panelRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const experimental = Boolean(getSetting("experimental_enabled"));
  const debug = Boolean(getSetting("debug_mode"));

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    panelRef.current?.focus();
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  useEffect(() => {
    contentRef.current?.scrollTo({ top: 0 });
  }, [tab]);

  if (!open) return null;

  const visible = (x: SettingsTab) =>
    (x !== "experimental" || experimental) && (x !== "debug" || debug);

  return createPortal(
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-overlay ev-modal-in"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-label={t("hubLayout.settings.title")}
        tabIndex={-1}
        data-testid="settings-modal"
        className="flex overflow-hidden bg-panel border border-hairline rounded-[12px] shadow-2xl outline-none"
        style={{ width: "min(960px, 94vw)", height: "min(640px, 90vh)" }}
      >
        <nav className="w-[192px] shrink-0 border-e border-hairline bg-background p-3 overflow-y-auto">
          <p className="px-2 pt-1 pb-3 text-[15px] font-semibold text-text">
            {t("hubLayout.settings.title")}
          </p>
          {SETTINGS_GROUPS.map((g) => {
            const tabs = g.tabs.filter(visible);
            if (tabs.length === 0) return null;
            return (
              <div key={g.key} className="mb-3">
                <p className="ev-label px-2 pb-1">{t(`hubLayout.settings.groups.${g.key}`)}</p>
                {tabs.map((x) => (
                  <button
                    key={x}
                    type="button"
                    data-tab={x}
                    aria-current={tab === x ? "page" : undefined}
                    onClick={() => onTabChange(x)}
                    className={`w-full text-start px-2 py-1.5 rounded-[6px] text-[14px] leading-5 cursor-pointer transition-colors ${
                      tab === x
                        ? "bg-nav-active text-text font-semibold"
                        : "text-mid-gray hover:bg-nav-hover hover:text-text"
                    }`}
                  >
                    {t(`hubLayout.settings.tabs.${x}`)}
                  </button>
                ))}
              </div>
            );
          })}
        </nav>
        <div className="flex-1 flex flex-col min-w-0">
          <div className="flex items-center justify-between px-6 pt-5 pb-3">
            <h2 className="text-[20px] leading-7 font-semibold text-text">
              {t(`hubLayout.settings.tabs.${tab}`)}
            </h2>
            <button
              type="button"
              onClick={onClose}
              aria-label={t("hubLayout.settings.close")}
              title={t("hubLayout.settings.close")}
              className="p-1.5 rounded-[6px] text-mid-gray hover:text-text hover:bg-nav-hover cursor-pointer"
            >
              <X className="w-5 h-5" />
            </button>
          </div>
          <div ref={contentRef} className="flex-1 overflow-y-auto px-6 pb-6">
            <TabContent tab={tab} />
          </div>
        </div>
      </div>
    </div>,
    document.body,
  );
};
