/**
 * Home dell'hub: saluto, due colonne
 * larghe al massimo 1046 px. A sinistra la cronologia per Oggi, Ieri e data,
 * con la ricerca e per riga copia, audio e menu (riprova, archivia, elimina).
 * A destra parole totali, parole al minuto, giorni di fila, poi le altre
 * statistiche e funzioni di prima: motori, avvisi, scorciatoie, prova della
 * pulizia (`cleanup_preview`), l'aquila di Eagle Voice.
 */
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { emit } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import { toast } from "sonner";
import {
  Archive,
  Check,
  Copy,
  MoreHorizontal,
  Pause,
  Play,
  RotateCcw,
  Search,
  Sparkles,
  Trash2,
} from "lucide-react";
import { commands, events, type HistoryEntry } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { useEngineState } from "@/hooks/useEngineState";
import { useSectionSettings } from "@/hooks/useSectionSettings";
import { useOsType } from "@/hooks/useOsType";
import { useDictationStats } from "@/hooks/useDictationStats";
import { formatKeyCombination } from "@/lib/utils/keyboard";
import {
  cleanupPreview,
  EMPTY_CLEANUP_CONTEXT,
  FLOW_EVENTS,
} from "@/lib/backend";
import { classifyNotices } from "@/lib/flow";
import {
  historyTexts,
  inHistoryView,
  matchesHistory,
  type FlowHistoryEntry,
} from "@/lib/history";
import { countWords, greetingKey, groupByDay, streakDays } from "@/lib/homeStats";
import {
  archiveHistoryEntry,
  copyHistoryEntry,
  deleteHistoryEntry,
  retryHistoryEntry,
} from "../settings/history/actions";
import { SettingsGroup } from "../ui/SettingsGroup";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { Textarea } from "../ui/Textarea";
import { EngineCard } from "./EngineCard";
import { NotConnectedBadge } from "./NotConnected";
import { FLOW_SHORTCUTS } from "./shortcuts";
import { EagleHero } from "./EagleHero";
import { HubPage } from "./HubPage";

const PAGE_SIZE = 100;

const RowButton: React.FC<{
  title: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}> = ({ title, onClick, disabled, children }) => (
  <button
    type="button"
    title={title}
    aria-label={title}
    disabled={disabled}
    onClick={onClick}
    className="p-1.5 rounded-[6px] text-mid-gray hover:text-text hover:bg-nav-hover cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
  >
    {children}
  </button>
);

/** Audio di una dettatura: un pulsante play e pausa, l'audio si carica al primo clic. */
const PlayButton: React.FC<{ fileName: string }> = ({ fileName }) => {
  const { t } = useTranslation();
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [playing, setPlaying] = useState(false);

  useEffect(() => () => audioRef.current?.pause(), []);

  const toggle = async () => {
    if (playing) {
      audioRef.current?.pause();
      return;
    }
    if (!audioRef.current) {
      const res = await commands.getAudioFilePath(fileName);
      if (res.status !== "ok") {
        toast.error(t("home.audioError"));
        return;
      }
      const audio = new Audio(convertFileSrc(res.data, "asset"));
      audio.onplay = () => setPlaying(true);
      audio.onpause = () => setPlaying(false);
      audio.onended = () => setPlaying(false);
      audioRef.current = audio;
    }
    audioRef.current.play().catch(() => toast.error(t("home.audioError")));
  };

  return (
    <RowButton title={playing ? t("home.pause") : t("home.play")} onClick={toggle}>
      {playing ? <Pause className="w-4 h-4" /> : <Play className="w-4 h-4" />}
    </RowButton>
  );
};

const RowMenu: React.FC<{
  onRetry: () => void;
  onArchive: () => void;
  onDelete: () => void;
}> = ({ onRetry, onArchive, onDelete }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);

  const item = (label: string, Icon: typeof RotateCcw, action: () => void) => (
    <button
      type="button"
      role="menuitem"
      onClick={() => {
        setOpen(false);
        action();
      }}
      className="flex items-center gap-2 w-full px-3 py-1.5 text-sm text-start text-text hover:bg-nav-hover cursor-pointer"
    >
      <Icon className="w-4 h-4 text-mid-gray" />
      {label}
    </button>
  );

  return (
    <div className="relative" ref={ref}>
      <RowButton title={t("home.more")} onClick={() => setOpen((o) => !o)}>
        <MoreHorizontal className="w-4 h-4" />
      </RowButton>
      {open && (
        <div
          role="menu"
          className="absolute end-0 top-full mt-1 z-20 min-w-[160px] py-1 rounded-[8px] border border-hairline bg-panel shadow-lg"
        >
          {item(t("home.retry"), RotateCcw, onRetry)}
          {item(t("home.archive"), Archive, onArchive)}
          {item(t("home.delete"), Trash2, onDelete)}
        </div>
      )}
    </div>
  );
};

const HistoryRow: React.FC<{
  entry: FlowHistoryEntry;
  onRemove: (id: number) => void;
  onReload: () => void;
}> = ({ entry, onRemove, onReload }) => {
  const { t, i18n } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const text = historyTexts(entry).final;
  const time = new Date(entry.timestamp * 1000).toLocaleTimeString(i18n.language, {
    hour: "2-digit",
    minute: "2-digit",
  });

  const copy = async () => {
    if (!(await copyHistoryEntry(entry))) {
      toast.error(t("settings.history.copyError"));
      return;
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const retry = async () => {
    setRetrying(true);
    try {
      await retryHistoryEntry(commands, entry.id);
    } catch {
      toast.error(t("settings.history.retranscribeError"));
    } finally {
      setRetrying(false);
    }
  };

  const archive = async () => {
    onRemove(entry.id);
    try {
      await archiveHistoryEntry(commands, entry.id, true);
    } catch {
      toast.error(t("hub.history.archiveError"));
      onReload();
    }
  };

  const remove = async () => {
    onRemove(entry.id);
    if (!(await deleteHistoryEntry(commands, entry.id))) onReload();
  };

  return (
    <div className="group flex gap-3 px-4 py-3" data-testid="home-history-row">
      <span className="w-12 shrink-0 pt-0.5 text-xs text-mid-gray tabular-nums">{time}</span>
      <div className="flex-1 min-w-0">
        <p
          className={`text-sm whitespace-pre-wrap break-words select-text ${
            retrying ? "text-mid-gray italic" : "text-text"
          }`}
        >
          {retrying ? t("settings.history.transcribing") : text || t("settings.history.transcriptionFailed")}
        </p>
        {entry.app_name && <p className="text-xs text-mid-gray mt-0.5">{entry.app_name}</p>}
      </div>
      <div className="flex items-start gap-0.5 shrink-0">
        <RowButton title={t("settings.history.copyToClipboard")} onClick={copy} disabled={!text}>
          {copied ? <Check className="w-4 h-4" /> : <Copy className="w-4 h-4" />}
        </RowButton>
        <PlayButton fileName={entry.file_name} />
        <RowMenu onRetry={retry} onArchive={archive} onDelete={remove} />
      </div>
    </div>
  );
};

const StatTile: React.FC<{ label: string; value: string; hint?: string; testId: string }> = ({
  label,
  value,
  hint,
  testId,
}) => (
  <div className="px-4 py-3" data-testid={testId}>
    <p className="text-xs text-mid-gray">{label}</p>
    <p className="text-2xl font-semibold tabular-nums text-text">{value}</p>
    {hint && <p className="text-xs text-mid-gray">{hint}</p>}
  </div>
);

export const HomeSection: React.FC = () => {
  const { t, i18n } = useTranslation();
  const osType = useOsType();
  const { settings } = useSettings();
  const engines = useEngineState();
  const cleanup = useSectionSettings("cleanup");
  const wpmStats = useDictationStats();
  const [entries, setEntries] = useState<FlowHistoryEntry[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [raw, setRaw] = useState("");
  const [preview, setPreview] = useState<string | null>(null);
  const [previewState, setPreviewState] = useState<
    "idle" | "running" | "notConnected" | "error"
  >("idle");

  const load = useCallback(async (cursor: number | null) => {
    try {
      const res = await commands.getHistoryEntries(cursor, PAGE_SIZE);
      if (res.status === "ok") {
        const next = res.data.entries as FlowHistoryEntry[];
        setEntries((prev) => {
          if (cursor === null) return next;
          const ids = new Set(prev.map((e) => e.id));
          return [...prev, ...next.filter((e) => !ids.has(e.id))];
        });
        setHasMore(res.data.has_more);
      }
    } catch (e) {
      console.error("Cronologia non leggibile:", e);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load(null);
    const unlisten = events.historyUpdatePayload.listen((event) => {
      const { action, entry } = event.payload as { action: string; entry: HistoryEntry };
      if (action === "added")
        setEntries((prev) => [entry as FlowHistoryEntry, ...prev.filter((e) => e.id !== entry.id)]);
      else if (action === "updated")
        setEntries((prev) => prev.map((e) => (e.id === entry.id ? (entry as FlowHistoryEntry) : e)));
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [load]);

  const active = useMemo(() => entries.filter((e) => inHistoryView(e, "list")), [entries]);
  const visible = useMemo(
    () => active.filter((e) => matchesHistory(e, query, null)),
    [active, query],
  );
  const groups = useMemo(() => groupByDay(visible, Date.now()), [visible]);
  const totalWords = useMemo(
    () => active.reduce((sum, e) => sum + countWords(historyTexts(e).final), 0),
    [active],
  );
  const streak = useMemo(() => streakDays(active.map((e) => e.timestamp), Date.now()), [active]);

  const notices = classifyNotices({
    recordingError: null,
    asr: engines.asr.state,
    cleanup: engines.cleanup.state,
    qualityMode: cleanup.value.quality_mode,
    silentForMs: 0,
  });

  const runPreview = async () => {
    if (!raw.trim()) return;
    setPreviewState("running");
    const res = await cleanupPreview(raw, {
      ...EMPTY_CLEANUP_CONTEXT,
      level: cleanup.value.level,
      format: cleanup.value.default_format,
    });
    if (res.ok) {
      setPreview(res.data);
      setPreviewState("idle");
    } else {
      setPreview(null);
      setPreviewState(res.notConnected ? "notConnected" : "error");
    }
  };

  const dayLabel = (g: { kind: string; day: number }) =>
    g.kind === "today"
      ? t("home.today")
      : g.kind === "yesterday"
        ? t("home.yesterday")
        : new Date(g.day).toLocaleDateString(i18n.language, {
            weekday: "long",
            day: "numeric",
            month: "long",
          });

  const nf = new Intl.NumberFormat(i18n.language);
  const wpm = wpmStats.averageWpm ?? wpmStats.lastWpm;

  return (
    <HubPage title={t(`home.greeting.${greetingKey(new Date().getHours())}`)} wide testId="page-home">
      <div className="grid gap-8 grid-cols-1 lg:grid-cols-[minmax(0,1.45fr)_minmax(0,1fr)]">
        {/* Colonna sinistra: cronologia */}
        <div className="min-w-0 space-y-3">
          <div className="relative">
            <Search className="w-4 h-4 absolute start-2.5 top-1/2 -translate-y-1/2 text-mid-gray" />
            <Input
              className="w-full ps-8"
              variant="compact"
              value={query}
              placeholder={t("home.search")}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
          {loading ? (
            <p className="px-1 py-3 text-sm text-mid-gray">{t("settings.history.loading")}</p>
          ) : groups.length === 0 ? (
            <p className="px-1 py-3 text-sm text-mid-gray">
              {active.length === 0 ? t("hub.home.noRecent") : t("hub.history.noMatch")}
            </p>
          ) : (
            groups.map((g) => (
              <div key={g.day} className="space-y-1.5">
                <h2 className="ev-label px-1 pt-2">{dayLabel(g)}</h2>
                <div className="bg-surface border border-hairline rounded-[14px] divide-y divide-hairline">
                  {g.items.map((e) => (
                    <HistoryRow
                      key={e.id}
                      entry={e}
                      onRemove={(id) => setEntries((prev) => prev.filter((x) => x.id !== id))}
                      onReload={() => load(null)}
                    />
                  ))}
                </div>
              </div>
            ))
          )}
          {hasMore && (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => load(entries[entries.length - 1]?.id ?? null)}
            >
              {t("home.loadMore")}
            </Button>
          )}
        </div>

        {/* Colonna destra: statistiche e il resto della Home di prima */}
        <div className="min-w-0 space-y-6">
          <SettingsGroup title={t("home.stats")}>
            <div className="grid grid-cols-3 divide-x divide-hairline">
              <StatTile testId="stat-words" label={t("home.totalWords")} value={nf.format(totalWords)} />
              <StatTile
                testId="wpm-average"
                label={t("home.wpm")}
                value={wpm === null ? "-" : String(wpm)}
                hint={
                  wpmStats.session.count > 0
                    ? t("hub.home.wpm.averageHint", { count: wpmStats.session.count })
                    : undefined
                }
              />
              <StatTile testId="stat-streak" label={t("home.streak")} value={String(streak)} />
            </div>
            <div className="grid grid-cols-2 divide-x divide-hairline">
              <StatTile
                testId="wpm-last"
                label={t("hub.home.wpm.last")}
                value={wpmStats.lastWpm === null ? "-" : String(wpmStats.lastWpm)}
              />
              <StatTile testId="stat-count" label={t("home.dictations")} value={nf.format(active.length)} />
            </div>
          </SettingsGroup>

          <div className="rounded-[14px] bg-cobalt text-white overflow-hidden">
            <EagleHero />
          </div>

          <SettingsGroup title={t("hub.home.engines")}>
            <EngineCard engine="asr" view={engines.asr} restarting={engines.restarting === "asr"} compact />
            <EngineCard
              engine="cleanup"
              view={engines.cleanup}
              restarting={engines.restarting === "cleanup"}
              compact
            />
          </SettingsGroup>

          {notices.length > 0 && (
            <SettingsGroup title={t("hub.home.notices")}>
              {notices.map((n) => (
                <div key={n.kind} className="flex items-center gap-3 px-4 py-2">
                  <span
                    className={`w-2 h-2 rounded-full ${
                      n.severity === "error" ? "bg-[#E5484D]" : "bg-warning"
                    }`}
                  />
                  <p className="flex-1 text-sm">{t(`notices.${n.kind}`, n.params)}</p>
                  {n.target && (
                    <Button
                      variant="secondary"
                      size="sm"
                      onClick={() => emit(FLOW_EVENTS.hubNavigate, { section: n.target })}
                    >
                      {t(`notices.action.${n.target}`)}
                    </Button>
                  )}
                </div>
              ))}
            </SettingsGroup>
          )}

          <SettingsGroup title={t("hub.home.shortcuts")}>
            {FLOW_SHORTCUTS.map((s) => {
              const binding = settings?.bindings?.[s.id];
              return (
                <div key={s.id} className="flex items-center justify-between gap-2 px-4 py-2">
                  <p className="text-sm">{t(`hub.shortcuts.${s.labelKey}`)}</p>
                  <div className="flex items-center gap-2">
                    {!binding && <NotConnectedBadge />}
                    <kbd className="px-2 py-0.5 text-xs font-semibold rounded-md bg-mid-gray/10 border border-mid-gray/30 whitespace-nowrap">
                      {binding ? formatKeyCombination(binding.current_binding, osType) : s.defaultCombo}
                    </kbd>
                  </div>
                </div>
              );
            })}
          </SettingsGroup>

          <SettingsGroup title={t("hub.home.tryCleanup")} description={t("hub.home.tryCleanupHint")}>
            <div className="px-4 py-3 space-y-3">
              <Textarea
                className="w-full"
                variant="compact"
                value={raw}
                placeholder={t("hub.home.tryCleanupPlaceholder")}
                onChange={(e) => setRaw(e.target.value)}
              />
              <div className="flex items-center gap-3">
                <Button
                  onClick={runPreview}
                  disabled={!raw.trim() || previewState === "running"}
                  className="flex items-center gap-2"
                >
                  <Sparkles className="w-4 h-4" />
                  {previewState === "running" ? t("hub.home.cleaning") : t("hub.home.tryCleanupButton")}
                </Button>
                {previewState === "notConnected" && <NotConnectedBadge />}
                {previewState === "error" && (
                  <span className="text-xs text-error">{t("hub.home.cleanupError")}</span>
                )}
              </div>
              {preview !== null && (
                <div className="rounded-xl border border-hairline bg-background px-3 py-2">
                  <p className="text-xs text-mid-gray mb-1">{t("hub.home.cleanedResult")}</p>
                  <p className="text-sm whitespace-pre-wrap select-text">{preview}</p>
                </div>
              )}
            </div>
          </SettingsGroup>
        </div>
      </div>
    </HubPage>
  );
};
