/**
 * Stili: livello di pulizia, formato di default e per app,
 * stile per categoria, interruttori backtrack, elenchi, "premi invio",
 * istruzioni Polish, modalità comando. Scrive `AppSettings.cleanup` tramite
 * `update_settings_section("cleanup", ...)`.
 */
import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Plus, Trash2 } from "lucide-react";
import { HubPage, HubTabs } from "../hub/HubPage";
import { useSectionSettings } from "@/hooks/useSectionSettings";
import type {
  CleanupLevel,
  StyleCategory,
  TranscriptionFormat,
  Tone,
} from "@/lib/backend";
import { SettingsGroup } from "../ui/SettingsGroup";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { Dropdown } from "../ui/Dropdown";
import { Input } from "../ui/Input";
import { Button } from "../ui/Button";
import { NotConnectedNote } from "../hub/NotConnected";

const LEVELS: CleanupLevel[] = ["exact", "tidy", "smooth", "rewrite"];
const FORMATS: TranscriptionFormat[] = ["smart", "raw", "chat", "lower"];
const CATEGORIES: StyleCategory[] = ["chat", "team", "mail", "general"];
const STYLES: Tone[] = [
  "default",
  "polished",
  "relaxed",
  "loose",
  "lively",
];

export const StylesSection: React.FC = () => {
  const { t } = useTranslation();
  const { value: s, update, notConnected } = useSectionSettings("cleanup");
  const [newApp, setNewApp] = useState("");
  const [category, setCategory] = useState<StyleCategory>("chat");
  const [newFormat, setNewFormat] = useState<TranscriptionFormat>("chat");

  const formatOptions = FORMATS.map((f) => ({
    value: f,
    label: t(`styles.format.options.${f}`),
  }));

  const addAppFormat = () => {
    const app = newApp.trim();
    if (!app) return;
    update({
      app_formats: [
        ...s.app_formats.filter(
          (r) => r.app_match.toLowerCase() !== app.toLowerCase(),
        ),
        { app_match: app, format: newFormat },
      ],
    });
    setNewApp("");
  };

  const current = s.styles[category];

  return (
    <HubPage
      title={t("sidebar.style")}
      testId="page-styles"
      subheader={
        <HubTabs<StyleCategory>
          tabs={CATEGORIES.map((cat) => ({
            id: cat,
            label: t(`styles.style.categories.${cat}`),
          }))}
          active={category}
          onChange={setCategory}
        />
      }
    >
    <div className="space-y-8">
      {notConnected && <NotConnectedNote what={t("sidebar.styles")} />}

      <div className="space-y-3">
        <p className="text-sm text-mid-gray">
          {t("stylePage.appliesTo", { apps: t(`styles.style.categoryHints.${category}`) })}
        </p>
        <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-3" role="radiogroup">
          {STYLES.map((v) => {
            const on = current === v;
            return (
              <button
                key={v}
                type="button"
                role="radio"
                aria-checked={on}
                data-style={v}
                onClick={() => update({ styles: { ...s.styles, [category]: v } })}
                className={`text-start rounded-[14px] border p-4 cursor-pointer transition-colors ${
                  on
                    ? "border-logo-primary bg-nav-active"
                    : "border-hairline bg-surface hover:bg-nav-hover"
                }`}
              >
                <p className="text-[15px] font-semibold text-text flex items-center justify-between gap-2">
                  {t(`styles.style.options.${v}`)}
                  {on && <Check className="w-4 h-4 text-logo-primary" />}
                </p>
                <p className="mt-1 text-xs text-mid-gray">{t(`stylePage.toneHints.${v}`)}</p>
                <p className="mt-3 text-sm text-text italic leading-snug">
                  {t(`stylePage.examples.${category}.${v}`)}
                </p>
              </button>
            );
          })}
        </div>
      </div>

      <h2 className="text-[17px] font-semibold text-text pt-2">{t("stylePage.moreRules")}</h2>

      <SettingsGroup title={t("styles.level.title")} description={t("styles.level.description")}>
        <div className="px-4 py-3 grid grid-cols-4 gap-2">
          {LEVELS.map((level) => (
            <button
              key={level}
              type="button"
              onClick={() => update({ level })}
              className={`rounded-lg border px-2 py-2 text-start cursor-pointer transition-colors ${
                s.level === level
                  ? "border-logo-primary bg-logo-primary/15"
                  : "border-hairline hover:bg-mid-gray/10"
              }`}
            >
              <p className="text-sm font-medium">{t(`styles.level.options.${level}`)}</p>
              <p className="text-[11px] text-mid-gray leading-snug">
                {t(`styles.level.hints.${level}`)}
              </p>
            </button>
          ))}
        </div>
      </SettingsGroup>

      <SettingsGroup title={t("styles.format.title")}>
        <SettingContainer
          title={t("styles.format.default")}
          description={t("styles.format.defaultDescription")}
          grouped
        >
          <Dropdown
            options={formatOptions}
            selectedValue={s.default_format}
            onSelect={(v) => update({ default_format: v as TranscriptionFormat })}
          />
        </SettingContainer>
        <div className="px-4 py-3 space-y-2">
          <p className="text-sm font-medium">{t("styles.format.perApp")}</p>
          {s.app_formats.length === 0 && (
            <p className="text-xs text-mid-gray">{t("styles.format.noApps")}</p>
          )}
          {s.app_formats.map((rule) => (
            <div key={rule.app_match} className="flex items-center gap-2">
              <span className="flex-1 text-sm truncate">{rule.app_match}</span>
              <Dropdown
                options={formatOptions}
                selectedValue={rule.format}
                onSelect={(v) =>
                  update({
                    app_formats: s.app_formats.map((r) =>
                      r.app_match === rule.app_match
                        ? { ...r, format: v as TranscriptionFormat }
                        : r,
                    ),
                  })
                }
              />
              <button
                type="button"
                className="p-1.5 text-mid-gray hover:text-logo-primary cursor-pointer"
                title={t("styles.format.removeApp")}
                aria-label={t("styles.format.removeApp")}
                onClick={() =>
                  update({
                    app_formats: s.app_formats.filter(
                      (r) => r.app_match !== rule.app_match,
                    ),
                  })
                }
              >
                <Trash2 className="w-4 h-4" />
              </button>
            </div>
          ))}
          <form
            className="flex items-center gap-2 pt-1"
            onSubmit={(e) => {
              e.preventDefault();
              addAppFormat();
            }}
          >
            <Input
              className="flex-1 min-w-0"
              variant="compact"
              value={newApp}
              placeholder={t("styles.format.appPlaceholder")}
              onChange={(e) => setNewApp(e.target.value)}
            />
            <Dropdown
              options={formatOptions}
              selectedValue={newFormat}
              onSelect={(v) => setNewFormat(v as TranscriptionFormat)}
            />
            <Button
              type="submit"
              variant="secondary"
              className="flex items-center gap-1 shrink-0"
            >
              <Plus className="w-4 h-4" />
              {t("styles.format.addApp")}
            </Button>
          </form>
        </div>
      </SettingsGroup>


      <SettingsGroup title={t("styles.behaviour.title")}>
        <ToggleSwitch
          grouped
          checked={s.backtrack_enabled}
          onChange={(v) => update({ backtrack_enabled: v })}
          label={t("styles.behaviour.backtrack")}
          description={t("styles.behaviour.backtrackDescription")}
        />
        <ToggleSwitch
          grouped
          checked={s.lists_enabled}
          onChange={(v) => update({ lists_enabled: v })}
          label={t("styles.behaviour.lists")}
          description={t("styles.behaviour.listsDescription")}
        />
        <ToggleSwitch
          grouped
          checked={s.press_enter_enabled}
          onChange={(v) => update({ press_enter_enabled: v })}
          label={t("styles.behaviour.pressEnter")}
          description={t("styles.behaviour.pressEnterDescription")}
        />
        <ToggleSwitch
          grouped
          checked={s.command_mode_enabled}
          onChange={(v) => update({ command_mode_enabled: v })}
          label={t("styles.behaviour.commandMode")}
          description={t("styles.behaviour.commandModeDescription")}
        />
      </SettingsGroup>

      <SettingsGroup title={t("styles.polish.title")} description={t("styles.polish.description")}>
        {s.polish_instructions.map((ins) => (
          <ToggleSwitch
            key={ins.id}
            grouped
            checked={ins.enabled}
            onChange={(v) =>
              update({
                polish_instructions: s.polish_instructions.map((x) =>
                  x.id === ins.id ? { ...x, enabled: v } : x,
                ),
              })
            }
            label={t(`styles.polish.items.${ins.id}`, { defaultValue: ins.label })}
            description={ins.label}
          />
        ))}
      </SettingsGroup>
    </div>
    </HubPage>
  );
};
