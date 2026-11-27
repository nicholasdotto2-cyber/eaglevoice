/**
 * Dizionario: parole, sostituzioni, snippet; ricerca, filtri, stella,
 * frequenza e ultimo uso; parole imparate da confermare; import da un file
 * JSON. Tutti i dati passano dai comandi `dictionary_*` del modulo cleanup.
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
  Check,
  FileDown,
  Pencil,
  Plus,
  Search,
  Star,
  Trash2,
  X,
} from "lucide-react";
import {
  dictionaryAcceptCandidate,
  dictionaryAdd,
  dictionaryCandidates,
  dictionaryDelete,
  dictionaryImportFromJson,
  dictionaryList,
  dictionaryRejectCandidate,
  dictionaryUpdate,
  deleteLearnedWord,
  FLOW_EVENTS,
  listLearnedWords,
  undoLearnedWord,
  type BackendResult,
  type DictionaryEntry,
  type LearnedCandidate,
  type LearnedWord,
} from "@/lib/backend";
import { HubPage, HubTabs } from "../hub/HubPage";
import { NotConnectedBadge } from "../hub/NotConnected";
import {
  entryKind,
  filterDictionary,
  validateDraft,
  type DictionaryDraft,
  type DictionaryFilter,
  type DictionaryKind,
  type DictionarySort,
} from "@/lib/dictionary";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { Dropdown } from "../ui/Dropdown";
import { SettingsGroup } from "../ui/SettingsGroup";
import { NotConnectedNote } from "../hub/NotConnected";

const FILTERS: DictionaryFilter[] = [
  "all",
  "word",
  "replacement",
  "snippet",
  "starred",
  "learned",
];
const KINDS: DictionaryKind[] = ["word", "replacement", "snippet"];
const EMPTY_DRAFT: DictionaryDraft = { kind: "word", phrase: "", replacement: "" };

const draftToEntry = (d: DictionaryDraft) => ({
  phrase: d.phrase.trim(),
  replacement: d.kind === "word" ? null : d.replacement.trim(),
  is_snippet: d.kind === "snippet",
});

const IconBtn: React.FC<{
  title: string;
  onClick: () => void;
  active?: boolean;
  children: React.ReactNode;
}> = ({ title, onClick, active, children }) => (
  <button
    type="button"
    title={title}
    aria-label={title}
    onClick={onClick}
    className={`p-1.5 rounded-md cursor-pointer transition-colors ${
      active ? "text-logo-primary" : "text-mid-gray hover:text-logo-primary"
    }`}
  >
    {children}
  </button>
);

const DraftForm: React.FC<{
  draft: DictionaryDraft;
  onChange: (d: DictionaryDraft) => void;
  onSubmit: () => void;
  onCancel?: () => void;
  submitLabel: string;
  error: string | null;
}> = ({ draft, onChange, onSubmit, onCancel, submitLabel, error }) => {
  const { t } = useTranslation();
  const kindOptions = KINDS.map((k) => ({ value: k, label: t(`dictionary.kind.${k}`) }));
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit();
      }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <Dropdown
          options={kindOptions}
          selectedValue={draft.kind}
          onSelect={(v) => onChange({ ...draft, kind: v as DictionaryKind })}
        />
        <Input
          className="flex-1 min-w-[140px]"
          variant="compact"
          value={draft.phrase}
          placeholder={t(`dictionary.placeholder.phrase.${draft.kind}`)}
          onChange={(e) => onChange({ ...draft, phrase: e.target.value })}
        />
        {draft.kind !== "word" && (
          <Input
            className="flex-1 min-w-[160px]"
            variant="compact"
            value={draft.replacement}
            placeholder={t(`dictionary.placeholder.replacement.${draft.kind}`)}
            onChange={(e) => onChange({ ...draft, replacement: e.target.value })}
          />
        )}
        <Button type="submit" size="md" className="flex items-center gap-1">
          <Plus className="w-4 h-4" />
          {submitLabel}
        </Button>
        {onCancel && (
          <Button type="button" variant="ghost" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
        )}
      </div>
      {error && <p className="text-xs text-error">{t(error)}</p>}
    </form>
  );
};

type DictionaryTab = "all" | "learned";

/** Snippet (pagina a sé): stessa lista filtrata sugli snippet. */
export const SnippetsSection: React.FC = () => <DictionarySection mode="snippets" />;

export const DictionarySection: React.FC<{ mode?: "dictionary" | "snippets" }> = ({
  mode = "dictionary",
}) => {
  const { t, i18n } = useTranslation();
  const snippets = mode === "snippets";
  const [entries, setEntries] = useState<DictionaryEntry[]>([]);
  const [candidates, setCandidates] = useState<LearnedCandidate[]>([]);
  const [learned, setLearned] = useState<LearnedWord[]>([]);
  const [learnedNotConnected, setLearnedNotConnected] = useState(false);
  const [tab, setTab] = useState<DictionaryTab>("all");
  const [addOpen, setAddOpen] = useState(false);
  const [notConnected, setNotConnected] = useState(false);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<DictionaryFilter>(snippets ? "snippet" : "all");
  const [sort, setSort] = useState<DictionarySort>("frequency");
  const [draft, setDraft] = useState<DictionaryDraft>(
    snippets ? { ...EMPTY_DRAFT, kind: "snippet" } : EMPTY_DRAFT,
  );
  const [draftError, setDraftError] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState<DictionaryDraft>(EMPTY_DRAFT);
  const [editError, setEditError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    const [list, cands, learnedRes] = await Promise.all([
      dictionaryList(),
      dictionaryCandidates(),
      listLearnedWords(),
    ]);
    if (list.ok) setEntries(list.data);
    if (cands.ok) setCandidates(cands.data);
    if (learnedRes.ok) setLearned(learnedRes.data);
    setLearnedNotConnected(!learnedRes.ok && learnedRes.notConnected);
    setNotConnected((!list.ok && list.notConnected) || (!cands.ok && cands.notConnected));
    setLoading(false);
  }, []);

  useEffect(() => {
    reload();
    const un = listen(FLOW_EVENTS.dictionaryChanged, () => reload());
    const unLearned = listen(FLOW_EVENTS.learned, () => reload());
    return () => {
      un.then((fn) => fn());
      unLearned.then((fn) => fn());
    };
  }, [reload]);

  const report = (res: BackendResult<unknown>, errorKey: string): boolean => {
    if (res.ok) return true;
    if (res.notConnected) setNotConnected(true);
    toast.error(res.notConnected ? t("hub.notConnected") : t(errorKey), {
      description: res.notConnected ? t("hub.notConnectedHint") : res.error,
    });
    return false;
  };

  const visible = useMemo(
    () => filterDictionary(entries, query, filter, sort),
    [entries, query, filter, sort],
  );

  const addEntry = async () => {
    const err = validateDraft(draft, entries);
    setDraftError(err);
    if (err) return;
    const res = await dictionaryAdd({ ...draftToEntry(draft), starred: false });
    if (report(res, "dictionary.errors.save") && res.ok) {
      setEntries((prev) => [...prev.filter((e) => e.id !== res.data.id), res.data]);
      setDraft({ ...EMPTY_DRAFT, kind: draft.kind });
    }
  };

  const saveEdit = async (entry: DictionaryEntry) => {
    const err = validateDraft(editDraft, entries, entry.id);
    setEditError(err);
    if (err) return;
    const next = { ...entry, ...draftToEntry(editDraft) };
    if (report(await dictionaryUpdate(next), "dictionary.errors.save")) {
      setEntries((prev) => prev.map((e) => (e.id === entry.id ? next : e)));
      setEditingId(null);
    }
  };

  const toggleStar = async (entry: DictionaryEntry) => {
    const next = { ...entry, starred: !entry.starred };
    if (report(await dictionaryUpdate(next), "dictionary.errors.save")) {
      setEntries((prev) => prev.map((e) => (e.id === entry.id ? next : e)));
    }
  };

  const remove = async (entry: DictionaryEntry) => {
    if (report(await dictionaryDelete(entry.id), "dictionary.errors.delete")) {
      setEntries((prev) => prev.filter((e) => e.id !== entry.id));
    }
  };

  const decide = async (c: LearnedCandidate, accept: boolean) => {
    const res = accept
      ? await dictionaryAcceptCandidate(c.id)
      : await dictionaryRejectCandidate(c.id);
    if (report(res, "dictionary.errors.save")) {
      setCandidates((prev) => prev.filter((x) => x.id !== c.id));
      if (accept) reload();
    }
  };

  const importFromJsonFile = async () => {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof path !== "string") return;
    const res = await dictionaryImportFromJson(path);
    if (report(res, "dictionary.errors.import") && res.ok) {
      toast.success(t("dictionary.imported", { count: res.data }));
      reload();
    }
  };

  const formatLastUsed = (value: string | null) => {
    if (!value) return t("dictionary.neverUsed");
    const d = new Date(value);
    return Number.isNaN(d.getTime())
      ? value
      : d.toLocaleDateString(i18n.language, { day: "numeric", month: "short" });
  };

  const sortOptions = (["frequency", "recent", "alpha"] as DictionarySort[]).map((s) => ({
    value: s,
    label: t(`dictionary.sort.${s}`),
  }));

  /** Annulla toglie la parola imparata e la sua sostituzione; Elimina toglie la riga. */
  const learnedAction = async (w: LearnedWord, action: "undo" | "delete") => {
    const res = action === "undo" ? await undoLearnedWord(w.id) : await deleteLearnedWord(w.id);
    if (res.ok) {
      // Dopo Annulla la voce resta con active falso: la rilettura la aggiorna.
      if (action === "delete") setLearned((prev) => prev.filter((x) => x.id !== w.id));
      reload();
      return;
    }
    if (res.notConnected) setLearnedNotConnected(true);
    toast.error(res.notConnected ? t("hub.notConnected") : t("dictionary.errors.save"), {
      description: res.notConnected ? t("hub.notConnectedHint") : res.error,
    });
  };

  const activeLearned = learned.filter((w) => w.active).length;
  const visibleList = snippets ? visible.filter((e) => entryKind(e) === "snippet") : visible;
  const total = snippets ? entries.filter((e) => entryKind(e) === "snippet").length : entries.length;

  const addForm = (
    <SettingsGroup title={snippets ? t("dictionaryPage.addSnippet") : t("dictionary.add")}>
      <div className="px-4 py-3">
        <DraftForm
          draft={draft}
          onChange={(d) => {
            setDraft(snippets ? { ...d, kind: "snippet" } : d);
            setDraftError(null);
          }}
          onSubmit={addEntry}
          onCancel={() => setAddOpen(false)}
          submitLabel={t("dictionary.addButton")}
          error={draftError}
        />
      </div>
    </SettingsGroup>
  );

  const candidatesGroup = candidates.length > 0 && (
    <SettingsGroup
      title={t("dictionary.candidates.title", { count: candidates.length })}
      description={t("dictionary.candidates.description")}
    >
      {candidates.map((c) => (
        <div key={c.id} className="flex items-center gap-3 px-4 py-2">
          <div className="flex-1 min-w-0">
            <p className="text-sm">
              <span className="line-through text-mid-gray">{c.wrong}</span>
              {"  "}
              <span className="font-semibold">{c.right}</span>
            </p>
            <p className="text-xs text-mid-gray truncate" title={c.example}>
              {t("dictionary.candidates.seen", { count: c.seen_count })}
              {c.example ? ` · ${c.example}` : ""}
            </p>
          </div>
          <Button
            size="sm"
            variant="primary-soft"
            className="flex items-center gap-1"
            onClick={() => decide(c, true)}
          >
            <Check className="w-3.5 h-3.5" />
            {t("dictionary.candidates.accept")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="flex items-center gap-1"
            onClick={() => decide(c, false)}
          >
            <X className="w-3.5 h-3.5" />
            {t("dictionary.candidates.reject")}
          </Button>
        </div>
      ))}
    </SettingsGroup>
  );

  const learnedTab = (
    <div className="space-y-8" data-testid="learned-tab">
      {learnedNotConnected && <NotConnectedNote what={t("dictionaryPage.learnedTab")} />}
      <div className="space-y-2">
        <p className="px-1 text-xs text-mid-gray">{t("dictionaryPage.learnedHint")}</p>
        <div className="bg-surface border border-hairline rounded-[14px] divide-y divide-hairline">
          {learned.length === 0 ? (
            <p className="px-4 py-3 text-sm text-mid-gray">{t("dictionaryPage.learnedEmpty")}</p>
          ) : (
            learned.map((w) => (
              <div
                key={w.id}
                className="flex items-center gap-3 px-4 py-2"
                data-testid="learned-row"
                data-active={w.active}
              >
                <div className="flex-1 min-w-0">
                  <p className={`text-sm ${w.active ? "" : "opacity-70"}`}>
                    <span className="line-through text-mid-gray">{w.wrong}</span>
                    <span className="text-mid-gray">{" → "}</span>
                    <span className={w.active ? "font-semibold text-text" : "text-mid-gray"}>
                      {w.right}
                    </span>
                  </p>
                  <p className="text-xs text-mid-gray">
                    {t("dictionaryPage.learnedCount", { count: w.count })}
                    {w.created_at ? ` · ${formatLastUsed(w.created_at)}` : ""}
                    {w.active ? "" : ` · ${t("dictionaryPage.undone")}`}
                  </p>
                </div>
                {w.active && (
                  <Button size="sm" variant="secondary" onClick={() => learnedAction(w, "undo")}>
                    {t("dictionaryPage.undo")}
                  </Button>
                )}
                <IconBtn title={t("dictionary.delete")} onClick={() => learnedAction(w, "delete")}>
                  <Trash2 className="w-4 h-4" />
                </IconBtn>
              </div>
            ))
          )}
        </div>
      </div>
      {candidatesGroup}
    </div>
  );

  const listTab = (
    <div className="space-y-6">
      {(addOpen || total === 0) && addForm}
      <div className="space-y-2">
        <div className="flex flex-wrap items-center gap-2">
          <div className="relative flex-1 min-w-[180px]">
            <Search className="w-4 h-4 absolute start-2.5 top-1/2 -translate-y-1/2 text-mid-gray" />
            <Input
              className="w-full ps-8"
              variant="compact"
              value={query}
              placeholder={snippets ? t("dictionaryPage.searchSnippets") : t("dictionary.search")}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
          <Dropdown
            options={sortOptions}
            selectedValue={sort}
            onSelect={(v) => setSort(v as DictionarySort)}
          />
        </div>
        {!snippets && (
          <div className="flex flex-wrap gap-1">
            {FILTERS.map((f) => (
              <button
                key={f}
                type="button"
                onClick={() => setFilter(f)}
                className={`px-2.5 py-1 rounded-full text-xs cursor-pointer border ${
                  filter === f
                    ? "bg-logo-primary text-on-primary border-logo-primary"
                    : "border-mid-gray/30 hover:bg-mid-gray/15"
                }`}
              >
                {t(`dictionary.filter.${f}`)}
              </button>
            ))}
          </div>
        )}
        <p className="px-1 text-xs text-mid-gray">
          {snippets
            ? t("dictionaryPage.snippetCount", { count: total })
            : t("dictionary.listTitle", { count: total })}
        </p>
        <div className="bg-surface border border-hairline rounded-[14px] divide-y divide-hairline">
          {loading ? (
            <p className="px-4 py-3 text-sm text-mid-gray">{t("dictionary.loading")}</p>
          ) : visibleList.length === 0 ? (
            <p className="px-4 py-3 text-sm text-mid-gray">
              {total === 0
                ? snippets
                  ? t("dictionaryPage.snippetsEmpty")
                  : t("dictionary.empty")
                : t("dictionary.noMatch")}
            </p>
          ) : (
            visibleList.map((entry) => {
              const kind = entryKind(entry);
              if (editingId === entry.id) {
                return (
                  <div key={entry.id} className="px-4 py-3">
                    <DraftForm
                      draft={editDraft}
                      onChange={(d) => {
                        setEditDraft(d);
                        setEditError(null);
                      }}
                      onSubmit={() => saveEdit(entry)}
                      onCancel={() => setEditingId(null)}
                      submitLabel={t("dictionary.save")}
                      error={editError}
                    />
                  </div>
                );
              }
              return (
                <div key={entry.id} className="flex items-center gap-3 px-4 py-2">
                  <IconBtn
                    title={entry.starred ? t("dictionary.unstar") : t("dictionary.star")}
                    active={entry.starred}
                    onClick={() => toggleStar(entry)}
                  >
                    <Star className="w-4 h-4" fill={entry.starred ? "currentColor" : "none"} />
                  </IconBtn>
                  <div className="flex-1 min-w-0">
                    <p className="text-sm font-medium break-words select-text whitespace-pre-wrap">
                      {entry.phrase}
                      {entry.replacement ? (
                        <span className="font-normal text-mid-gray">
                          {kind === "snippet" ? " ⇒ " : " → "}
                          <span className="text-text">{entry.replacement}</span>
                        </span>
                      ) : null}
                    </p>
                    <p className="text-xs text-mid-gray">
                      {t(`dictionary.kind.${kind}`)}
                      {entry.source !== "manual"
                        ? ` · ${t(`dictionary.source.${entry.source}`)}`
                        : ""}
                      {` · ${t("dictionary.frequency", { count: entry.frequency })}`}
                      {` · ${t("dictionary.lastUsed", { date: formatLastUsed(entry.last_used) })}`}
                    </p>
                  </div>
                  <IconBtn
                    title={t("dictionary.edit")}
                    onClick={() => {
                      setEditingId(entry.id);
                      setEditError(null);
                      setEditDraft({
                        kind,
                        phrase: entry.phrase,
                        replacement: entry.replacement ?? "",
                      });
                    }}
                  >
                    <Pencil className="w-4 h-4" />
                  </IconBtn>
                  <IconBtn title={t("dictionary.delete")} onClick={() => remove(entry)}>
                    <Trash2 className="w-4 h-4" />
                  </IconBtn>
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );

  return (
    <HubPage
      title={snippets ? t("sidebar.snippets") : t("sidebar.dictionary")}
      testId={snippets ? "page-snippets" : "page-dictionary"}
      actions={
        <>
          {!snippets && (
            <Button
              variant="secondary"
              size="md"
              className="flex items-center gap-2"
              onClick={importFromJsonFile}
            >
              <FileDown className="w-4 h-4" />
              {t("dictionaryPage.import")}
            </Button>
          )}
          <Button
            size="md"
            className="flex items-center gap-2"
            data-testid="dictionary-add"
            onClick={() => {
              setTab("all");
              setAddOpen(true);
            }}
          >
            <Plus className="w-4 h-4" />
            {t("dictionaryPage.add")}
          </Button>
        </>
      }
      subheader={
        snippets ? undefined : (
          <HubTabs<DictionaryTab>
            tabs={[
              { id: "all", label: t("dictionaryPage.allTab") },
              {
                id: "learned",
                label: (
                  <span className="inline-flex items-center gap-2">
                    {t("dictionaryPage.learnedTab")}
                    {activeLearned + candidates.length > 0 && (
                      <span className="px-1.5 rounded-full text-[11px] bg-nav-active text-text">
                        {activeLearned + candidates.length}
                      </span>
                    )}
                    {learnedNotConnected && <NotConnectedBadge />}
                  </span>
                ),
              },
            ]}
            active={tab}
            onChange={setTab}
          />
        )
      }
    >
      {notConnected && (
        <div className="mb-4">
          <NotConnectedNote what={t("sidebar.dictionary")} />
        </div>
      )}
      {tab === "learned" && !snippets ? learnedTab : listTab}
    </HubPage>
  );
};

