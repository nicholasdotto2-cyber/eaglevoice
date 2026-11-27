import React from "react";
import { useTranslation } from "react-i18next";
import { PlugZap } from "lucide-react";

/** Etichetta "non collegato": il comando Rust che serve a questa parte non esiste ancora. */
export const NotConnectedBadge: React.FC<{ title?: string }> = ({ title }) => {
  const { t } = useTranslation();
  return (
    <span
      className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[11px] font-medium bg-mid-gray/15 text-mid-gray"
      title={title ?? t("hub.notConnectedHint")}
    >
      <PlugZap className="w-3 h-3" />
      {t("hub.notConnected")}
    </span>
  );
};

/** Riga di avviso in testa a una sezione che lavora senza backend. */
export const NotConnectedNote: React.FC<{ what: string }> = ({ what }) => {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-2 px-4 py-2 rounded-lg border border-dashed border-mid-gray/40 text-xs text-mid-gray">
      <NotConnectedBadge />
      <span>{t("hub.notConnectedNote", { what })}</span>
    </div>
  );
};
