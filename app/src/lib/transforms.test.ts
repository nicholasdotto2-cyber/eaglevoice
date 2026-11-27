import { describe, expect, test } from "bun:test";
import {
  canCreate,
  examplesUnlocked,
  nextFreeSlot,
  validateTransform,
  type TransformDraft,
} from "./transforms";
import { transformBindingId } from "./backend";

const words = (n: number) => Array.from({ length: n }, (_, i) => `parola${i}`).join(" ");
const slots = (n: number) => Array.from({ length: n }, (_, i) => ({ slot: i + 1 }));
const draft = (over: Partial<TransformDraft> = {}): TransformDraft => ({
  slot: 4,
  name: "Email cordiale",
  shortcut: "",
  prompt: "Riscrivi come email.",
  examples: [],
  builtin: false,
  ...over,
});

describe("Transforms: slot, esempi, validazione", () => {
  test("Crea nuovo sparisce a 9 slot; primo slot libero dal 3", () => {
    expect(canCreate(slots(8))).toBe(true);
    expect(canCreate(slots(9))).toBe(false);
    expect(nextFreeSlot([{ slot: 1 }, { slot: 2 }, { slot: 4 }])).toBe(3);
  });
  test("gli esempi compaiono solo con nome, scorciatoia (binding) e prompt pieni", () => {
    expect(examplesUnlocked({ name: "A", prompt: "B", shortcut: "super+alt+4" })).toBe(true);
    expect(examplesUnlocked({ name: "A", prompt: "  ", shortcut: "super+alt+4" })).toBe(false);
    // Slot 4-9 partono senza scorciatoia.
    expect(examplesUnlocked({ name: "A", prompt: "B", shortcut: "" })).toBe(false);
  });
  test("esempi da 50 a 500 parole, al massimo 5; nome e prompt obbligatori", () => {
    expect(validateTransform(draft({ examples: [words(50), words(500)] }))).toBeNull();
    expect(validateTransform(draft({ examples: [words(49)] }))).toBe("exampleLength");
    expect(validateTransform(draft({ examples: [words(501)] }))).toBe("exampleLength");
    expect(validateTransform(draft({ examples: Array(6).fill(words(60)) }))).toBe("tooManyExamples");
    expect(validateTransform(draft({ name: " " }))).toBe("name");
    expect(validateTransform(draft({ prompt: "" }))).toBe("prompt");
    expect(validateTransform(draft({ prompt: "", builtin: true }))).toBeNull();
  });
  test("binding della scorciatoia: slot 1 polish, slot N transform_N", () => {
    expect(transformBindingId(1)).toBe("polish");
    expect(transformBindingId(2)).toBe("transform_2");
    expect(transformBindingId(9)).toBe("transform_9");
  });
});
