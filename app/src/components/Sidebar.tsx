/**
 * Barra laterale dell'hub: 216 px
 * (60 compressa), padding 12; voci testo 15/20, icona 20, padding 8, raggio 6,
 * gap 4; voce attiva e hover su un neutro chiaro. In alto il logo e il pulsante
 * per comprimere; Alt+Su e Alt+Giù cambiano pagina. In fondo Impostazioni
 * (finestra modale) e Aiuto.
 */
import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import {
  BookA,
  CircleHelp,
  FlaskConical,
  History,
  House,
  Palette,
  PanelLeftClose,
  PanelLeftOpen,
  Scissors,
  Settings,
  Wand2,
} from "lucide-react";
import EagleVoiceLogo from "./icons/EagleVoiceLogo";
import { useSettings } from "../hooks/useSettings";
import { DebugSettings } from "./settings";
import { HomeSection } from "./hub/HomeSection";
import { HistoryPage } from "./hub/HistoryPage";
import { DictionarySection, SnippetsSection } from "./dictionary/DictionarySection";
import { StylesSection } from "./styles/StylesSection";
import { TransformsSection } from "./transforms/TransformsSection";
import { SettingsModal, tabForSection, type SettingsTab } from "./hub/SettingsModal";
import { FLOW_EVENTS, type HubNavigateEvent } from "@/lib/backend";
import { nextSection } from "@/lib/hubNav";

export type SidebarSection = keyof typeof SECTIONS_CONFIG;

interface IconProps {
  width?: number | string;
  height?: number | string;
  size?: number | string;
  className?: string;
  [key: string]: any;
}

interface SectionConfig {
  labelKey: string;
  icon: React.ComponentType<IconProps>;
  component: React.ComponentType;
  enabled: (settings: any) => boolean;
}

// Pagine dell'hub, in ordine. La chiave `general`
// resta (App.tsx parte da lì e la usa come ripiego) ma mostra la Home. Le
// impostazioni, i motori, le avanzate e le info stanno nella finestra modale.
export const SECTIONS_CONFIG = {
  general: {
    labelKey: "sidebar.home",
    icon: House,
    component: HomeSection,
    enabled: () => true,
  },
  dictionary: {
    labelKey: "sidebar.dictionary",
    icon: BookA,
    component: DictionarySection,
    enabled: () => true,
  },
  snippets: {
    labelKey: "sidebar.snippets",
    icon: Scissors,
    component: SnippetsSection,
    enabled: () => true,
  },
  styles: {
    labelKey: "sidebar.style",
    icon: Palette,
    component: StylesSection,
    enabled: () => true,
  },
  transforms: {
    labelKey: "sidebar.transforms",
    icon: Wand2,
    component: TransformsSection,
    enabled: () => true,
  },
  history: {
    labelKey: "sidebar.history",
    icon: History,
    component: HistoryPage,
    enabled: () => true,
  },
  debug: {
    labelKey: "sidebar.debug",
    icon: FlaskConical,
    component: DebugSettings,
    enabled: (settings) => settings?.debug_mode ?? false,
  },
} as const satisfies Record<string, SectionConfig>;

const isSection = (value: string): value is SidebarSection =>
  Object.prototype.hasOwnProperty.call(SECTIONS_CONFIG, value);

const COLLAPSE_KEY = "eagle.sidebarCollapsed";

interface SidebarProps {
  activeSection: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
}

const NavItem: React.FC<{
  icon: React.ComponentType<IconProps>;
  label: string;
  active?: boolean;
  collapsed: boolean;
  onClick: () => void;
  id: string;
}> = ({ icon: Icon, label, active, collapsed, onClick, id }) => (
  <button
    type="button"
    data-section={id}
    aria-current={active ? "page" : undefined}
    title={collapsed ? label : undefined}
    onClick={onClick}
    className={`flex items-center gap-2 p-2 w-full rounded-[6px] text-[15px] leading-[20px] cursor-pointer transition-colors ${
      collapsed ? "justify-center" : ""
    } ${
      active
        ? "bg-nav-active text-text font-semibold"
        : "text-mid-gray hover:bg-nav-hover hover:text-text"
    }`}
  >
    <Icon width={20} height={20} className="shrink-0" />
    {!collapsed && <span className="truncate">{label}</span>}
  </button>
);

export const Sidebar: React.FC<SidebarProps> = ({
  activeSection,
  onSectionChange,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return localStorage.getItem(COLLAPSE_KEY) === "1";
    } catch {
      return false;
    }
  });
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("shortcuts");

  const pages = Object.entries(SECTIONS_CONFIG)
    .filter(([id, config]) => id !== "debug" && config.enabled(settings))
    .map(([id, config]) => ({ id: id as SidebarSection, ...config }));

  const openSettings = useCallback((tab: SettingsTab) => {
    setSettingsTab(tab);
    setSettingsOpen(true);
  }, []);

  // La barra di dettatura (o la home) chiede di aprire una sezione: `hub:navigate`.
  // Le sezioni che oggi stanno nelle Impostazioni aprono la finestra modale.
  useEffect(() => {
    const unlisten = listen<HubNavigateEvent>(FLOW_EVENTS.hubNavigate, (e) => {
      const section = e.payload?.section;
      if (!section) return;
      const tab = tabForSection(section);
      if (tab && tab !== "debug") openSettings(tab);
      else if (isSection(section)) {
        setSettingsOpen(false);
        onSectionChange(section);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [onSectionChange, openSettings]);

  // Alt+Su e Alt+Giù: pagina precedente e successiva.
  const pageIds = pages.map((p) => p.id).join(",");
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.altKey || settingsOpen) return;
      if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
      e.preventDefault();
      const ids = pageIds.split(",") as SidebarSection[];
      onSectionChange(nextSection(ids, activeSection, e.key === "ArrowDown" ? 1 : -1));
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [pageIds, activeSection, onSectionChange, settingsOpen]);

  const toggleCollapsed = () => {
    setCollapsed((c) => {
      try {
        localStorage.setItem(COLLAPSE_KEY, c ? "0" : "1");
      } catch {
        // solo per questa sessione
      }
      return !c;
    });
  };

  const CollapseIcon = collapsed ? PanelLeftOpen : PanelLeftClose;

  return (
    <div
      data-testid="hub-sidebar"
      className="flex flex-col h-full shrink-0 bg-background p-3 transition-[width] duration-200 ease-out"
      style={{ width: collapsed ? 60 : 216 }}
    >
      <div
        className={`flex items-center mb-4 ${collapsed ? "flex-col gap-2" : "justify-between"}`}
      >
        <EagleVoiceLogo size={collapsed ? 28 : 32} className={collapsed ? "" : "px-1"} />
        <button
          type="button"
          onClick={toggleCollapsed}
          aria-label={collapsed ? t("hubLayout.expand") : t("hubLayout.collapse")}
          title={collapsed ? t("hubLayout.expand") : t("hubLayout.collapse")}
          className="p-1.5 rounded-[6px] text-mid-gray hover:text-text hover:bg-nav-hover cursor-pointer"
        >
          <CollapseIcon width={18} height={18} />
        </button>
      </div>
      <nav className="flex flex-col gap-1" aria-label={t("hubLayout.nav")}>
        {pages.map((p) => (
          <NavItem
            key={p.id}
            id={p.id}
            icon={p.icon}
            label={t(p.labelKey)}
            active={activeSection === p.id && !settingsOpen}
            collapsed={collapsed}
            onClick={() => onSectionChange(p.id)}
          />
        ))}
      </nav>
      <div className="mt-auto flex flex-col gap-1 pt-3">
        <NavItem
          id="settings"
          icon={Settings}
          label={t("sidebar.settings")}
          active={settingsOpen}
          collapsed={collapsed}
          onClick={() => openSettings(settingsTab)}
        />
        <NavItem
          id="help"
          icon={CircleHelp}
          label={t("sidebar.help")}
          collapsed={collapsed}
          onClick={() => openSettings("about")}
        />
      </div>
      <SettingsModal
        open={settingsOpen}
        tab={settingsTab}
        onTabChange={(tab) => {
          // Debug apre la sua pagina: lì c'è anche l'anteprima dell'onboarding (App.tsx).
          if (tab === "debug") {
            setSettingsOpen(false);
            onSectionChange("debug");
          } else setSettingsTab(tab);
        }}
        onClose={() => setSettingsOpen(false)}
      />
    </div>
  );
};
