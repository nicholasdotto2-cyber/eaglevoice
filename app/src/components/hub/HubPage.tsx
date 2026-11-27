/**
 * Pannello di una pagina dell'hub, con queste misure: bordo,
 * raggio 24/24/16/16, padding 40 40 0, larghezza massima max(800px, 50vw)
 * (la Home arriva a 1046 px di contenuto), titolo h1 24/32 con le azioni a destra.
 */
import React from "react";

interface HubPageProps {
  title: React.ReactNode;
  /** Pulsanti a destra del titolo. */
  actions?: React.ReactNode;
  /** Sotto il titolo, prima del contenuto (per esempio le schede). */
  subheader?: React.ReactNode;
  /** Pagina larga (Home a due colonne). */
  wide?: boolean;
  children: React.ReactNode;
  testId?: string;
}

export const HubPage: React.FC<HubPageProps> = ({
  title,
  actions,
  subheader,
  wide,
  children,
  testId,
}) => (
  <section
    data-testid={testId}
    className="ev-panel w-full bg-panel border border-hairline"
    style={{
      maxWidth: wide ? "calc(1046px + 80px)" : "max(800px, 50vw)",
      borderRadius: "24px 24px 16px 16px",
      padding: "40px 40px 0",
    }}
  >
    <header className="flex items-center justify-between gap-4 mb-6">
      <h1 className="text-[24px] leading-[32px] font-semibold text-text">{title}</h1>
      {actions && <div className="flex items-center gap-2 shrink-0">{actions}</div>}
    </header>
    {subheader}
    <div className="pb-10">{children}</div>
  </section>
);

/** Schede sotto il titolo (Dizionario, Stile). */
export const HubTabs = <T extends string>({
  tabs,
  active,
  onChange,
}: {
  tabs: Array<{ id: T; label: React.ReactNode }>;
  active: T;
  onChange: (id: T) => void;
}) => (
  <div role="tablist" className="flex flex-wrap gap-1 mb-6 border-b border-hairline">
    {tabs.map((tab) => (
      <button
        key={tab.id}
        type="button"
        role="tab"
        data-tab={tab.id}
        aria-selected={active === tab.id}
        onClick={() => onChange(tab.id)}
        className={`-mb-px px-3 py-2 text-[14px] leading-5 border-b-2 cursor-pointer transition-colors ${
          active === tab.id
            ? "border-logo-primary text-text font-semibold"
            : "border-transparent text-mid-gray hover:text-text"
        }`}
      >
        {tab.label}
      </button>
    ))}
  </div>
);
