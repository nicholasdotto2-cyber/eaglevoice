import { describe, expect, test } from "bun:test";
import { entryKind, filterDictionary, validateDraft } from "./dictionary";
import type { DictionaryEntry } from "./backend";

const e = (over: Partial<DictionaryEntry>): DictionaryEntry => ({
  id: "x",
  phrase: "x",
  replacement: null,
  is_snippet: false,
  starred: false,
  frequency: 0,
  last_used: null,
  source: "manual",
  ...over,
});

const entries: DictionaryEntry[] = [
  e({ id: "1", phrase: "Nomeprova", frequency: 12, last_used: "2026-10-10T10:00:00Z" }),
  e({ id: "2", phrase: "demi", replacement: "Demo", frequency: 40, source: "learned" }),
  e({ id: "3", phrase: "firma", replacement: "Nome Esempio, Ditta Prova", is_snippet: true, frequency: 3 }),
  e({ id: "4", phrase: "GitHub", starred: true, frequency: 1, last_used: "2026-10-17T10:00:00Z" }),
  e({ id: "5", phrase: "perché", frequency: 5 }),
];
const ids = (list: DictionaryEntry[]) => list.map((x) => x.id);

describe("dizionario", () => {
  test("tipo della voce", () => {
    expect(entryKind(entries[0])).toBe("word");
    expect(entryKind(entries[1])).toBe("replacement");
    expect(entryKind(entries[2])).toBe("snippet");
  });
  test("stellate prima, poi per frequenza", () => {
    expect(ids(filterDictionary(entries, ""))).toEqual(["4", "2", "1", "5", "3"]);
  });
  test("ordine per ultimo uso e alfabetico", () => {
    expect(ids(filterDictionary(entries, "", "all", "recent")).slice(0, 2)).toEqual(["4", "1"]);
    expect(ids(filterDictionary(entries, "", "word", "alpha"))).toEqual(["4", "1", "5"]);
  });
  test("ricerca su frase e sostituzione, senza maiuscole e accenti", () => {
    expect(ids(filterDictionary(entries, "demo"))).toEqual(["2"]);
    expect(ids(filterDictionary(entries, "PERCHE"))).toEqual(["5"]);
    expect(ids(filterDictionary(entries, "ditta"))).toEqual(["3"]);
    expect(filterDictionary(entries, "zzz")).toHaveLength(0);
  });
  test("filtri per tipo, stella, imparate", () => {
    expect(ids(filterDictionary(entries, "", "replacement"))).toEqual(["2"]);
    expect(ids(filterDictionary(entries, "", "snippet"))).toEqual(["3"]);
    expect(ids(filterDictionary(entries, "", "starred"))).toEqual(["4"]);
    expect(ids(filterDictionary(entries, "", "learned"))).toEqual(["2"]);
  });
  test("validazione della voce a mano", () => {
    expect(validateDraft({ kind: "word", phrase: "  ", replacement: "" }, entries)).toBe("dictionary.errors.emptyPhrase");
    expect(validateDraft({ kind: "snippet", phrase: "saluti", replacement: " " }, entries)).toBe("dictionary.errors.emptyReplacement");
    expect(validateDraft({ kind: "word", phrase: "nomeprova", replacement: "" }, entries)).toBe("dictionary.errors.duplicate");
    expect(validateDraft({ kind: "word", phrase: "Nomeprova", replacement: "" }, entries, "1")).toBeNull();
    expect(validateDraft({ kind: "word", phrase: "Python", replacement: "" }, entries)).toBeNull();
  });
});
