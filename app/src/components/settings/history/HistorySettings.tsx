import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { readFile } from "@tauri-apps/plugin-fs";
import {
  Archive,
  ArchiveRestore,
  Check,
  Copy,
  FolderOpen,
  RotateCcw,
  Search,
  Star,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  commands,
  events,
  type HistoryEntry,
  type HistoryUpdatePayload,
} from "@/bindings";
import { useOsType } from "@/hooks/useOsType";
import { formatDateTime } from "@/utils/dateFormat";
import { AudioPlayer, AudioPlayerGroup } from "../../ui/AudioPlayer";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";
import { Dropdown } from "../../ui/Dropdown";
import {
  archiveHistoryEntry,
  copyHistoryEntry,
  deleteHistoryEntry,
  retryHistoryEntry as retryHistoryEntryCommand,
} from "./actions";
import {
  historyTexts,
  inHistoryView,
  matchesHistory,
  type FlowHistoryEntry,
  type HistoryView,
} from "@/lib/history";

const ALL_APPS = "__all__";

const IconButton: React.FC<{
  onClick: () => void;
  title: string;
  disabled?: boolean;
  active?: boolean;
  children: React.ReactNode;
}> = ({ onClick, title, disabled, active, children }) => (
  <button
    onClick={onClick}
    disabled={disabled}
    className={`p-1.5 rounded-md flex items-center justify-center transition-colors cursor-pointer disabled:cursor-not-allowed disabled:text-text/20 ${
      active
        ? "text-logo-primary hover:underline"
        : "text-mid-gray hover:text-logo-primary"
    }`}
    title={title}
  >
    {children}
  </button>
);

const PAGE_SIZE = 30;

interface OpenRecordingsButtonProps {
  onClick: () => void;
  label: string;
}

const OpenRecordingsButton: React.FC<OpenRecordingsButtonProps> = ({
  onClick,
  label,
}) => (
  <Button
    onClick={onClick}
    variant="secondary"
    size="sm"
    className="flex items-center gap-2"
    title={label}
  >
    <FolderOpen className="w-4 h-4" />
    <span>{label}</span>
  </Button>
);

export const HistorySettings: React.FC = () => {
  const { t } = useTranslation();
  const osType = useOsType();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(true);
  const sentinelRef = useRef<HTMLDivElement>(null);
  const entriesRef = useRef<HistoryEntry[]>([]);
  const loadingRef = useRef(false);
  const [query, setQuery] = useState("");
  const [appFilter, setAppFilter] = useState<string>(ALL_APPS);
  const [view, setView] = useState<HistoryView>("list");

  const appNames = useMemo(
    () =>
      Array.from(
        new Set(
          (entries as FlowHistoryEntry[])
            .map((e) => e.app_name)
            .filter((x): x is string => !!x),
        ),
      ).sort(),
    [entries],
  );
  const visibleEntries = useMemo(
    () =>
      (entries as FlowHistoryEntry[]).filter(
        (e) =>
          inHistoryView(e, view) &&
          matchesHistory(e, query, appFilter === ALL_APPS ? null : appFilter),
      ),
    [entries, query, appFilter, view],
  );

  // Keep ref in sync for use in IntersectionObserver callback
  useEffect(() => {
    entriesRef.current = entries;
  }, [entries]);

  const loadPage = useCallback(async (cursor?: number) => {
    const isFirstPage = cursor === undefined;
    if (!isFirstPage && loadingRef.current) return;
    loadingRef.current = true;

    if (isFirstPage) setLoading(true);

    try {
      const result = await commands.getHistoryEntries(
        cursor ?? null,
        PAGE_SIZE,
      );
      if (result.status === "ok") {
        const { entries: newEntries, has_more } = result.data;
        setEntries((prev) => {
          if (isFirstPage) return newEntries;
          const existingIds = new Set(prev.map((entry) => entry.id));
          return [
            ...prev,
            ...newEntries.filter((entry) => !existingIds.has(entry.id)),
          ];
        });
        setHasMore(has_more);
      }
    } catch (error) {
      console.error("Failed to load history entries:", error);
    } finally {
      setLoading(false);
      loadingRef.current = false;
    }
  }, []);

  // Initial load
  useEffect(() => {
    loadPage();
  }, [loadPage]);

  // Infinite scroll via IntersectionObserver
  useEffect(() => {
    if (loading) return;

    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore) return;

    const observer = new IntersectionObserver(
      (observerEntries) => {
        const first = observerEntries[0];
        if (first.isIntersecting) {
          const lastEntry = entriesRef.current[entriesRef.current.length - 1];
          if (lastEntry) {
            loadPage(lastEntry.id);
          }
        }
      },
      { threshold: 0 },
    );

    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [loading, hasMore, loadPage]);

  // Listen for new entries added from the transcription pipeline
  useEffect(() => {
    const unlisten = events.historyUpdatePayload.listen((event) => {
      const payload: HistoryUpdatePayload = event.payload;
      if (payload.action === "added") {
        setEntries((prev) => [
          payload.entry,
          ...prev.filter((entry) => entry.id !== payload.entry.id),
        ]);
      } else if (payload.action === "updated") {
        setEntries((prev) =>
          prev.map((e) => (e.id === payload.entry.id ? payload.entry : e)),
        );
      }
      // "deleted" and "toggled" are handled by optimistic updates only,
      // so we intentionally ignore them here to avoid double-mutation.
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const toggleSaved = async (id: number) => {
    // Optimistic update
    setEntries((prev) =>
      prev.map((e) => (e.id === id ? { ...e, saved: !e.saved } : e)),
    );
    try {
      const result = await commands.toggleHistoryEntrySaved(id);
      if (result.status !== "ok") {
        // Revert on failure
        setEntries((prev) =>
          prev.map((e) => (e.id === id ? { ...e, saved: !e.saved } : e)),
        );
      }
    } catch (error) {
      console.error("Failed to toggle saved status:", error);
      // Revert on failure
      setEntries((prev) =>
        prev.map((e) => (e.id === id ? { ...e, saved: !e.saved } : e)),
      );
    }
  };

  const getAudioUrl = useCallback(
    async (fileName: string) => {
      try {
        const result = await commands.getAudioFilePath(fileName);
        if (result.status === "ok") {
          if (osType === "linux") {
            const fileData = await readFile(result.data);
            const blob = new Blob([fileData], { type: "audio/wav" });
            return URL.createObjectURL(blob);
          }
          return convertFileSrc(result.data, "asset");
        }
        return null;
      } catch (error) {
        console.error("Failed to get audio file path:", error);
        return null;
      }
    },
    [osType],
  );

  const deleteAudioEntry = async (id: number) => {
    // Optimistically remove
    setEntries((prev) => prev.filter((e) => e.id !== id));
    // Reload on failure
    if (!(await deleteHistoryEntry(commands, id))) loadPage();
  };

  const retryHistoryEntry = (id: number) =>
    retryHistoryEntryCommand(commands, id);

  const setArchived = async (id: number, archived: boolean) => {
    // Aggiornamento ottimistico; l'evento "updated" del backend conferma la voce.
    setEntries((prev) => prev.map((e) => (e.id === id ? { ...e, archived } : e)));
    try {
      await archiveHistoryEntry(commands, id, archived);
    } catch (error) {
      console.error("Failed to archive entry:", error);
      toast.error(t("hub.history.archiveError"));
      setEntries((prev) =>
        prev.map((e) => (e.id === id ? { ...e, archived: !archived } : e)),
      );
    }
  };

  const openRecordingsFolder = async () => {
    try {
      const result = await commands.openRecordingsFolder();
      if (result.status !== "ok") {
        throw new Error(String(result.error));
      }
    } catch (error) {
      console.error("Failed to open recordings folder:", error);
    }
  };

  let content: React.ReactNode;

  if (loading) {
    content = (
      <div className="px-4 py-3 text-center text-mid-gray">
        {t("settings.history.loading")}
      </div>
    );
  } else if (entries.length === 0) {
    content = (
      <div className="px-4 py-3 text-center text-mid-gray">
        {t("settings.history.empty")}
      </div>
    );
  } else if (visibleEntries.length === 0) {
    content = (
      <>
        <div className="px-4 py-3 text-center text-mid-gray">
          {view === "archived" && !query && appFilter === ALL_APPS
            ? t("hub.history.archivedEmpty")
            : t("hub.history.noMatch")}
        </div>
        {/* Le voci filtrate possono stare nelle pagine successive: lo scorrimento continua. */}
        <div ref={sentinelRef} className="h-1" />
      </>
    );
  } else {
    content = (
      <>
        <AudioPlayerGroup>
          <div className="divide-y divide-hairline">
            {visibleEntries.map((entry) => (
              <HistoryEntryComponent
                key={entry.id}
                entry={entry}
                onToggleSaved={() => toggleSaved(entry.id)}
                onCopyText={() => copyHistoryEntry(entry)}
                getAudioUrl={getAudioUrl}
                deleteAudio={deleteAudioEntry}
                retryTranscription={retryHistoryEntry}
                onSetArchived={(archived) => setArchived(entry.id, archived)}
              />
            ))}
          </div>
        </AudioPlayerGroup>
        {/* Sentinel for infinite scroll */}
        <div ref={sentinelRef} className="h-1" />
      </>
    );
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-8">
      <div className="space-y-2">
        <div className="px-4 flex items-center justify-between">
          <div>
            <h2 className="text-[13px] font-semibold text-text">
              {t("settings.history.title")}
            </h2>
          </div>
          <OpenRecordingsButton
            onClick={openRecordingsFolder}
            label={t("settings.history.openFolder")}
          />
        </div>
        <div className="flex flex-wrap items-center gap-2 px-1">
          <div className="relative flex-1 min-w-[180px]">
            <Search className="w-4 h-4 absolute start-2.5 top-1/2 -translate-y-1/2 text-mid-gray" />
            <Input
              className="w-full ps-8"
              variant="compact"
              value={query}
              placeholder={t("hub.history.search")}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
          <Dropdown
            options={[
              { value: ALL_APPS, label: t("hub.history.allApps") },
              ...appNames.map((a) => ({ value: a, label: a })),
            ]}
            selectedValue={appFilter}
            onSelect={setAppFilter}
          />
          <Dropdown
            options={[
              { value: "list", label: t("hub.history.views.list") },
              { value: "archived", label: t("hub.history.views.archived") },
            ]}
            selectedValue={view}
            onSelect={(v) => setView(v as HistoryView)}
          />
        </div>
        <div className="bg-surface border border-hairline rounded-[14px] overflow-visible">
          {content}
        </div>
      </div>
    </div>
  );
};

interface HistoryEntryProps {
  entry: HistoryEntry;
  onToggleSaved: () => void;
  onCopyText: () => Promise<boolean>;
  getAudioUrl: (fileName: string) => Promise<string | null>;
  deleteAudio: (id: number) => Promise<void>;
  retryTranscription: (id: number) => Promise<void>;
  onSetArchived: (archived: boolean) => void;
}

const HistoryEntryComponent: React.FC<HistoryEntryProps> = ({
  entry,
  onToggleSaved,
  onCopyText,
  getAudioUrl,
  deleteAudio,
  retryTranscription,
  onSetArchived,
}) => {
  const { t, i18n } = useTranslation();
  const [showCopied, setShowCopied] = useState(false);
  const [retrying, setRetrying] = useState(false);

  const flowEntry = entry as FlowHistoryEntry;
  const texts = historyTexts(flowEntry);
  const hasTranscription = texts.final.trim().length > 0;
  // Testo grezzo, pulito e corretto a mano: si mostrano solo quelli diversi dal testo principale.
  const variants = [
    { key: "raw", text: texts.raw },
    { key: "cleaned", text: texts.cleaned },
    { key: "edited", text: texts.edited },
  ].filter(
    (v): v is { key: string; text: string } =>
      !!v.text && v.text.trim().length > 0,
  );
  const meta = [
    flowEntry.app_name,
    flowEntry.category
      ? t(`hub.history.category.${flowEntry.category}`, {
          defaultValue: flowEntry.category,
        })
      : null,
    typeof flowEntry.latency_ms === "number"
      ? t("hub.history.latency", {
          seconds: (flowEntry.latency_ms / 1000).toFixed(1),
        })
      : null,
    flowEntry.used_quality ? t("hub.history.quality") : null,
    // lingua rilevata o scelta per questa dettatura, se salvata.
    flowEntry.language
      ? t(`hub.settings.languages.names.${flowEntry.language}`, {
          defaultValue: flowEntry.language,
        })
      : null,
  ].filter((x): x is string => !!x);

  const handleLoadAudio = useCallback(
    () => getAudioUrl(entry.file_name),
    [getAudioUrl, entry.file_name],
  );

  const handleCopyText = async () => {
    if (!hasTranscription) {
      return;
    }

    const copied = await onCopyText();
    if (!copied) {
      toast.error(t("settings.history.copyError"));
      return;
    }

    setShowCopied(true);
    setTimeout(() => setShowCopied(false), 2000);
  };

  const handleDeleteEntry = async () => {
    try {
      await deleteAudio(entry.id);
    } catch (error) {
      console.error("Failed to delete entry:", error);
      toast.error(t("settings.history.deleteError"));
    }
  };

  const handleRetranscribe = async () => {
    try {
      setRetrying(true);
      await retryTranscription(entry.id);
    } catch (error) {
      console.error("Failed to re-transcribe:", error);
      toast.error(t("settings.history.retranscribeError"));
    } finally {
      setRetrying(false);
    }
  };

  const formattedDate = formatDateTime(String(entry.timestamp), i18n.language);

  return (
    <div className="px-4 py-2 pb-5 flex flex-col gap-3">
      <div className="flex justify-between items-center">
        <p className="text-sm font-medium">{formattedDate}</p>
        <div className="flex items-center">
          <IconButton
            onClick={handleCopyText}
            disabled={!hasTranscription || retrying}
            title={t("settings.history.copyToClipboard")}
          >
            {showCopied ? (
              <Check width={16} height={16} />
            ) : (
              <Copy width={16} height={16} />
            )}
          </IconButton>
          <IconButton
            onClick={onToggleSaved}
            disabled={retrying}
            active={entry.saved}
            title={
              entry.saved
                ? t("settings.history.unsave")
                : t("settings.history.save")
            }
          >
            <Star
              width={16}
              height={16}
              fill={entry.saved ? "currentColor" : "none"}
            />
          </IconButton>
          <IconButton
            onClick={handleRetranscribe}
            disabled={retrying}
            title={t("settings.history.retranscribe")}
          >
            <RotateCcw
              width={16}
              height={16}
              style={
                retrying
                  ? { animation: "spin 1s linear infinite reverse" }
                  : undefined
              }
            />
          </IconButton>
          <IconButton
            onClick={() => onSetArchived(!entry.archived)}
            disabled={retrying}
            title={
              entry.archived
                ? t("hub.history.unarchive")
                : t("hub.history.archive")
            }
          >
            {entry.archived ? (
              <ArchiveRestore width={16} height={16} />
            ) : (
              <Archive width={16} height={16} />
            )}
          </IconButton>
          <IconButton
            onClick={handleDeleteEntry}
            disabled={retrying}
            title={t("settings.history.delete")}
          >
            <Trash2 width={16} height={16} />
          </IconButton>
        </div>
      </div>

      <p
        className={`italic text-sm pb-2 ${
          retrying
            ? ""
            : hasTranscription
              ? "text-text/90 select-text cursor-text whitespace-pre-wrap break-words"
              : "text-mid-gray"
        }`}
        style={
          retrying
            ? { animation: "transcribe-pulse 3s ease-in-out infinite" }
            : undefined
        }
      >
        {retrying && (
          <style>{`
            @keyframes transcribe-pulse {
              0%, 100% { color: color-mix(in srgb, var(--color-text) 40%, transparent); }
              50% { color: color-mix(in srgb, var(--color-text) 90%, transparent); }
            }
          `}</style>
        )}
        {retrying
          ? t("settings.history.transcribing")
          : hasTranscription
            ? texts.final
            : t("settings.history.transcriptionFailed")}
      </p>

      {meta.length > 0 && (
        <p className="text-xs text-mid-gray -mt-3">{meta.join(" · ")}</p>
      )}

      {!retrying && variants.length > 1 && (
        <details className="text-xs -mt-2">
          <summary className="cursor-pointer text-mid-gray hover:text-logo-primary select-none">
            {t("hub.history.showVersions")}
          </summary>
          <div className="mt-2 space-y-2">
            {variants.map((v) => (
              <div key={v.key}>
                <p className="text-[11px] uppercase tracking-wide text-mid-gray">
                  {t(`hub.history.versions.${v.key}`)}
                </p>
                <p className="text-sm text-text/80 whitespace-pre-wrap break-words select-text">
                  {v.text}
                </p>
              </div>
            ))}
          </div>
        </details>
      )}

      <AudioPlayer onLoadRequest={handleLoadAudio} className="w-full" />
    </div>
  );
};
