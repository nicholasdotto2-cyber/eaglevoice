import { describe, expect, test } from "bun:test";
import { mergeRankedMicrophones, moveItem } from "./microphones";

describe("microfoni in ordine di preferenza", () => {
  test("prima la classifica salvata, poi i presenti nuovi; niente Default né doppioni", () => {
    const list = mergeRankedMicrophones(
      ["Cuffie USB", "Microfono da tavolo", "Cuffie USB"],
      ["Default", "Microfono Realtek", "Cuffie USB"],
    );
    expect(list).toEqual([
      { name: "Cuffie USB", present: true },
      { name: "Microfono da tavolo", present: false },
      { name: "Microfono Realtek", present: true },
    ]);
  });
  test("su e giù; ai bordi non cambia nulla", () => {
    expect(moveItem(["a", "b", "c"], 2, -1)).toEqual(["a", "c", "b"]);
    expect(moveItem(["a", "b", "c"], 0, 1)).toEqual(["b", "a", "c"]);
    expect(moveItem(["a", "b"], 0, -1)).toEqual(["a", "b"]);
    expect(moveItem(["a", "b"], 1, 1)).toEqual(["a", "b"]);
  });
});
