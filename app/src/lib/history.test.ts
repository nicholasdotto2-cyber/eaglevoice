import { describe, expect, test } from "bun:test";
import { inHistoryView } from "./history";

describe("cronologia: lista e archiviate", () => {
  test("le archiviate stanno solo nella vista Archiviate; senza campo valgono come non archiviate", () => {
    const entries = [
      { id: 1, archived: false },
      { id: 2, archived: true },
      { id: 3 },
    ];
    expect(entries.filter((e) => inHistoryView(e, "list")).map((e) => e.id)).toEqual([1, 3]);
    expect(entries.filter((e) => inHistoryView(e, "archived")).map((e) => e.id)).toEqual([2]);
  });
});
