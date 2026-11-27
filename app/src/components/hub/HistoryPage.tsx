/**
 * Cronologia completa nel pannello dell'hub: filtro per app, archivio, versioni
 * grezza, pulita e corretta, preferiti. La Home mostra le dettature per giorno;
 * qui resta tutto ciò che c'era prima.
 */
import React from "react";
import { useTranslation } from "react-i18next";
import { HistorySettings } from "../settings/history/HistorySettings";
import { HubPage } from "./HubPage";

export const HistoryPage: React.FC = () => {
  const { t } = useTranslation();
  return (
    <HubPage title={t("sidebar.history")} testId="page-history">
      <HistorySettings />
    </HubPage>
  );
};
