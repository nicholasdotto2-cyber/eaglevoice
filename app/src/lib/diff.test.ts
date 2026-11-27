import { describe, expect, test } from "bun:test";
import { countChanges, wordDiff } from "./diff";

describe("diff a parole del Transform", () => {
  test("una sostituzione e un'aggiunta: due modifiche, i due testi si ricompongono", () => {
    const before = "alfa beta uno due tre quattro cinque";
    const after = "Alfa Beta, uno due tre quattro extra cinque";
    const segs = wordDiff(before, after);
    expect(countChanges(segs)).toBe(2);
    const rebuiltAfter = segs.filter((s) => s.kind !== "del").map((s) => s.text).join("");
    const rebuiltBefore = segs.filter((s) => s.kind !== "ins").map((s) => s.text).join("");
    expect(rebuiltAfter).toBe(after);
    expect(rebuiltBefore).toBe(before);
  });
  test("testi uguali: nessuna modifica", () => {
    expect(countChanges(wordDiff("uno due", "uno due"))).toBe(0);
  });
});
