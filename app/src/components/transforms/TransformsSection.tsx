/**
 * Transforms: interruttore generale, "Applica automaticamente dopo la
 * dettatura" con la scelta del Transform, card di Polish, Prompt Builder e dei
 * personalizzati (titolo, sottotitolo, tasti), "Crea nuovo" che sparisce a 9
 * slot, "Ripristina valori predefiniti". Editor a due pannelli: nome,
 * scorciatoia e prompt a sinistra; a destra fino a 5 esempi (50-500 parole),
 * che compaiono quando i tre campi sono pieni.
 *
 * Comandi: get_transforms, save_transform, delete_transform, reset_transforms,
 * set_transforms_options (lib/backend.ts, dai bindings). La scorciatoia di uno
 * slot è il binding `polish` (slot 1) o `transform_N` (2-9), cambiato con i
 * comandi delle scorciatoie di sempre.
 */
import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ArrowLeft, Plus, RotateCcw, Trash2 } from "lucide-react";
import {
  deleteTransform,
  getTransforms,
  resetTransforms,
  saveTransform,
  setTransformsOptions,
  transformBindingId,
  type BackendResult,
  type TransformsState,
  type TransformSlot,
} from "@/lib/backend";
import {
  canCreate,
  draftToSlot,
  emptyDraft,
  EXAMPLE_MAX_WORDS,
  EXAMPLE_MIN_WORDS,
  exampleOk,
  examplesUnlocked,
  MAX_EXAMPLES,
  validateTransform,
  wordCount,
  type TransformDraft,
} from "@/lib/transforms";
import { HubPage } from "../hub/HubPage";
import { NotConnectedNote } from "../hub/NotConnected";
import { SettingsGroup } from "../ui/SettingsGroup";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { SettingContainer } from "../ui/SettingContainer";
import { Dropdown } from "../ui/Dropdown";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { Textarea } from "../ui/Textarea";
import { ShortcutInput } from "../settings/ShortcutInput";
import { useSettings } from "@/hooks/useSettings";
import { formatKeyCombination } from "@/lib/utils/keyboard";
import { compactShortcut } from "@/lib/flow";

/** Stato vuoto finché `get_transforms` non risponde. */
const EMPTY: TransformsState = {
  enabled: true,
  auto_apply: { active: false, slot: 1 },
  slots: [],
  max_words: 3000,
  local_max_words: 3000,
};

/** Tasti della scorciatoia (binding `polish` o `transform_N`); vuota: "Nessuna". */
const Keys: React.FC<{ binding: string }> = ({ binding }) => {
  const { t } = useTranslation();
  if (!binding.trim())
    return <span className="text-xs text-mid-gray">{t("transforms.noShortcut")}</span>;
  return (
    <span className="flex flex-wrap gap-1">
      {compactShortcut(formatKeyCombination(binding, "windows"))
        .split("+")
        .map((k) => k.trim())
        .filter(Boolean)
        .map((k, i) => (
          <kbd
            key={i}
            className="px-1.5 py-0.5 text-[11px] font-semibold rounded-md bg-mid-gray/10 border border-mid-gray/30 text-text"
          >
            {k}
          </kbd>
        ))}
    </span>
  );
};

const subtitle = (slot: TransformSlot, t: (k: string) => string) => {
  if (slot.builtin && slot.slot === 1) return t("transforms.polishSubtitle");
  if (slot.builtin && slot.slot === 2) return t("transforms.promptEngineerSubtitle");
  return slot.prompt.split("\n")[0] || t("transforms.customSubtitle");
};

/** Nome da mostrare: quello di Prompt Builder è tradotto, gli altri restano come salvati. */
const slotName = (slot: TransformSlot, t: (k: string) => string) =>
  slot.builtin && slot.slot === 2 ? t("transforms.promptBuilderName") : slot.name;

const Editor: React.FC<{
  initial: TransformDraft;
  bindingOf: (slot: number) => string;
  onSave: (slot: TransformSlot) => Promise<boolean>;
  onDelete?: () => Promise<boolean>;
  onClose: () => void;
}> = ({ initial, bindingOf, onSave, onDelete, onClose }) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const [d, setD] = useState<TransformDraft>(initial);
  const [error, setError] = useState<string | null>(null);
  const bindingId = transformBindingId(d.slot);
  const hasBinding = Boolean(settings?.bindings?.[bindingId]);
  const unlocked = examplesUnlocked({
    name: d.name,
    prompt: d.builtin ? "fisso" : d.prompt,
    shortcut: bindingOf(d.slot),
  });

  const save = async () => {
    const err = validateTransform(d);
    setError(err);
    if (err) return;
    if (await onSave(draftToSlot(d))) onClose();
  };

  return (
    <div className="space-y-4" data-testid="transform-editor">
      <button
        type="button"
        onClick={onClose}
        className="flex items-center gap-1 text-sm text-mid-gray hover:text-text cursor-pointer"
      >
        <ArrowLeft className="w-4 h-4" />
        {t("transforms.editor.back")}
      </button>
      <div className="grid gap-6 grid-cols-1 lg:grid-cols-2">
        <div className="space-y-4 bg-surface border border-hairline rounded-[14px] p-5">
          <label className="block space-y-1.5">
            <span className="text-sm font-medium text-text">{t("transforms.editor.name")}</span>
            <Input
              className="w-full"
              variant="compact"
              value={d.name}
              disabled={d.builtin}
              placeholder={t("transforms.editor.namePlaceholder")}
              onChange={(e) => setD({ ...d, name: e.target.value })}
            />
          </label>
          <label className="block space-y-1.5">
            <span className="text-sm font-medium text-text">{t("transforms.editor.shortcut")}</span>
          </label>
          {hasBinding ? (
            <div className="-mx-1 rounded-[10px] border border-hairline" data-testid="transform-shortcut">
              <ShortcutInput shortcutId={bindingId} descriptionMode="tooltip" grouped />
              {!bindingOf(d.slot).trim() && (
                <p className="px-4 pb-2 text-xs text-mid-gray">{t("transforms.noShortcutHint")}</p>
              )}
            </div>
          ) : (
            <p className="text-sm text-mid-gray" data-testid="transform-shortcut">
              {t("transforms.noShortcut")}
            </p>
          )}
          <label className="block space-y-1.5">
            <span className="text-sm font-medium text-text">{t("transforms.editor.prompt")}</span>
            {d.builtin ? (
              <p className="text-sm text-mid-gray">{t("transforms.editor.builtinPrompt")}</p>
            ) : (
              <Textarea
                className="w-full min-h-[180px]"
                value={d.prompt}
                placeholder={t("transforms.editor.promptPlaceholder")}
                onChange={(e) => setD({ ...d, prompt: e.target.value })}
              />
            )}
          </label>
        </div>
        <div className="space-y-3 bg-surface border border-hairline rounded-[14px] p-5">
          <div>
            <p className="text-sm font-medium text-text">{t("transforms.editor.examples")}</p>
            <p className="text-xs text-mid-gray">
              {t("transforms.editor.examplesHint", {
                min: EXAMPLE_MIN_WORDS,
                max: EXAMPLE_MAX_WORDS,
                count: MAX_EXAMPLES,
              })}
            </p>
          </div>
          {!unlocked ? (
            <p className="text-sm text-mid-gray" data-testid="examples-locked">
              {t("transforms.editor.examplesLocked")}
            </p>
          ) : (
            <>
              {d.examples.map((ex, i) => {
                const n = wordCount(ex);
                return (
                  <div key={i} className="space-y-1">
                    <Textarea
                      className="w-full min-h-[90px]"
                      variant="compact"
                      value={ex}
                      placeholder={t("transforms.editor.examplePlaceholder")}
                      onChange={(e) =>
                        setD({
                          ...d,
                          examples: d.examples.map((x, j) => (j === i ? e.target.value : x)),
                        })
                      }
                    />
                    <div className="flex items-center justify-between text-xs">
                      <span className={ex.trim() && !exampleOk(ex) ? "text-error" : "text-mid-gray"}>
                        {t("transforms.editor.words", { count: n })}
                      </span>
                      <button
                        type="button"
                        className="text-mid-gray hover:text-text cursor-pointer"
                        onClick={() => setD({ ...d, examples: d.examples.filter((_, j) => j !== i) })}
                      >
                        {t("transforms.editor.removeExample")}
                      </button>
                    </div>
                  </div>
                );
              })}
              {d.examples.length < MAX_EXAMPLES && (
                <Button
                  variant="secondary"
                  size="sm"
                  className="flex items-center gap-1"
                  onClick={() => setD({ ...d, examples: [...d.examples, ""] })}
                >
                  <Plus className="w-4 h-4" />
                  {t("transforms.editor.addExample")}
                </Button>
              )}
            </>
          )}
        </div>
      </div>
      <div className="flex items-center gap-2">
        <Button onClick={save} data-testid="transform-save">
          {t("transforms.editor.save")}
        </Button>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        {onDelete && (
          <Button
            variant="ghost"
            className="flex items-center gap-1 ms-auto"
            onClick={async () => {
              if (await onDelete()) onClose();
            }}
          >
            <Trash2 className="w-4 h-4" />
            {t("transforms.editor.delete")}
          </Button>
        )}
        {error && <span className="text-xs text-error">{t(`transforms.errors.${error}`)}</span>}
      </div>
    </div>
  );
};

export const TransformsSection: React.FC = () => {
  const { t } = useTranslation();
  const [state, setState] = useState<TransformsState>(EMPTY);
  const [notConnected, setNotConnected] = useState(false);
  const [editing, setEditing] = useState<TransformDraft | null>(null);

  const { settings } = useSettings();
  const bindingOf = (slot: number) =>
    settings?.bindings?.[transformBindingId(slot)]?.current_binding ?? "";

  /** Rilegge lo stato dal backend: save, delete, reset e opzioni non lo restituiscono. */
  const reload = useCallback(async () => {
    const res = await getTransforms();
    if (res.ok) {
      setState(res.data);
      setNotConnected(false);
    } else {
      setNotConnected(res.notConnected);
      if (!res.notConnected) toast.error(t("transforms.errors.load"), { description: res.error });
    }
  }, [t]);

  /** Esito di un comando: in errore il messaggio del backend così com'è, poi si rilegge. */
  const done = async (res: BackendResult<unknown>): Promise<boolean> => {
    if (!res.ok) {
      if (res.notConnected) setNotConnected(true);
      toast.error(t("transforms.errors.save"), { description: res.error });
    }
    await reload();
    return res.ok;
  };

  useEffect(() => {
    reload();
  }, [reload]);

  const setOptions = async (patch: { enabled?: boolean; active?: boolean; slot?: number }) =>
    done(
      await setTransformsOptions(
        patch.enabled ?? state.enabled,
        patch.active ?? state.auto_apply.active,
        patch.slot ?? state.auto_apply.slot,
      ),
    );

  const save = async (slot: TransformSlot) => done(await saveTransform(slot));
  const remove = async (slotNumber: number) => done(await deleteTransform(slotNumber));

  const reset = async () => {
    if (!window.confirm(t("transforms.resetConfirm"))) return;
    await done(await resetTransforms());
  };

  const openEditor = (s: TransformSlot) =>
    setEditing({
      slot: s.slot,
      name: s.name,
      shortcut: s.shortcut,
      prompt: s.prompt,
      examples: [...s.examples],
      builtin: s.builtin,
    });

  return (
    <HubPage
      title={t("sidebar.transforms")}
      testId="page-transforms"
      actions={
        !editing && (
          <Button variant="secondary" className="flex items-center gap-2" onClick={reset}>
            <RotateCcw className="w-4 h-4" />
            {t("transforms.reset")}
          </Button>
        )
      }
    >
      {notConnected && (
        <div className="mb-4">
          <NotConnectedNote what={t("sidebar.transforms")} />
        </div>
      )}
      {editing ? (
        <Editor
          initial={editing}
          bindingOf={bindingOf}
          onSave={save}
          onDelete={editing.builtin || !state.slots.some((s) => s.slot === editing.slot)
            ? undefined
            : () => remove(editing.slot)}
          onClose={() => setEditing(null)}
        />
      ) : (
        <div className="space-y-8">
          <SettingsGroup>
            <ToggleSwitch
              grouped
              checked={state.enabled}
              onChange={(enabled) => setOptions({ enabled })}
              label={t("transforms.enabled")}
              description={t("transforms.enabledDescription")}
              descriptionMode="inline"
            />
            <ToggleSwitch
              grouped
              checked={state.auto_apply.active}
              onChange={(active) => setOptions({ active })}
              disabled={!state.enabled}
              label={t("transforms.autoApply")}
              description={t("transforms.autoApplyDescription")}
              descriptionMode="inline"
            />
            <SettingContainer
              title={t("transforms.autoApplyWhich")}
              description={t("transforms.autoApplyWhichDescription")}
              descriptionMode="inline"
              grouped
              disabled={!state.enabled || !state.auto_apply.active}
            >
              <Dropdown
                options={state.slots.map((s) => ({ value: String(s.slot), label: slotName(s, t) }))}
                selectedValue={String(state.auto_apply.slot)}
                onSelect={(v) => setOptions({ slot: Number(v) })}
                disabled={!state.enabled || !state.auto_apply.active}
              />
            </SettingContainer>
          </SettingsGroup>
          <p className="px-1 -mt-5 text-xs text-mid-gray" data-testid="transform-limits">
            {t("transforms.limits", { max: state.max_words, local: state.local_max_words })}
          </p>

          <div className="grid gap-3 grid-cols-1 sm:grid-cols-2 xl:grid-cols-3">
            {state.slots.map((s) => (
              <button
                key={s.slot}
                type="button"
                data-slot={s.slot}
                onClick={() => openEditor(s)}
                className="text-start rounded-[14px] border border-hairline bg-surface p-4 cursor-pointer hover:bg-nav-hover transition-colors flex flex-col gap-3 min-h-[128px]"
              >
                <div>
                  <p className="text-[15px] font-semibold text-text">{slotName(s, t)}</p>
                  <p className="text-xs text-mid-gray line-clamp-2 mt-0.5">{subtitle(s, t)}</p>
                </div>
                <div className="mt-auto">
                  <Keys binding={bindingOf(s.slot)} />
                </div>
              </button>
            ))}
            {canCreate(state.slots) && (
              <button
                type="button"
                data-testid="transform-create"
                onClick={() => setEditing(emptyDraft(state.slots))}
                className="rounded-[14px] border border-dashed border-mid-gray/50 p-4 cursor-pointer hover:bg-nav-hover transition-colors flex flex-col items-center justify-center gap-2 min-h-[128px] text-mid-gray hover:text-text"
              >
                <Plus className="w-5 h-5" />
                <span className="text-sm font-medium">{t("transforms.create")}</span>
              </button>
            )}
          </div>
        </div>
      )}
    </HubPage>
  );
};
