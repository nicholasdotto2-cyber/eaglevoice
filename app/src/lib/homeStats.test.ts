import { describe, expect, test } from "bun:test";
import { countWords, greetingKey, groupByDay, streakDays } from "./homeStats";
import { nextSection } from "./hubNav";

const at = (y: number, m: number, d: number, h = 12) => new Date(y, m - 1, d, h).getTime();
const sec = (ms: number) => Math.floor(ms / 1000);

describe("Home: cronologia per giorno e statistiche", () => {
  const now = at(2026, 11, 18, 10);
  test("Oggi, Ieri, poi la data, dal più recente", () => {
    const groups = groupByDay(
      [
        { id: 1, timestamp: sec(at(2026, 11, 18, 9)) },
        { id: 2, timestamp: sec(at(2026, 11, 17, 22)) },
        { id: 3, timestamp: sec(at(2026, 11, 14, 8)) },
        { id: 4, timestamp: sec(at(2026, 11, 18, 1)) },
      ],
      now,
    );
    expect(groups.map((g) => g.kind)).toEqual(["today", "yesterday", "date"]);
    expect(groups[0].items.map((i) => i.id)).toEqual([1, 4]);
  });
  test("giorni di fila: la serie che finisce ieri vale ancora, un buco la interrompe", () => {
    const ts = [at(2026, 11, 17), at(2026, 11, 16), at(2026, 11, 15), at(2026, 11, 13)].map(sec);
    expect(streakDays(ts, now)).toBe(3);
    expect(streakDays([sec(at(2026, 11, 18, 8)), ...ts], now)).toBe(4);
    expect(streakDays([sec(at(2026, 11, 10))], now)).toBe(0);
  });
  test("parole e saluto", () => {
    expect(countWords("  Parola uno, parola due  parola tre ")).toBe(6);
    expect(countWords("")).toBe(0);
    expect(greetingKey(9)).toBe("morning");
    expect(greetingKey(15)).toBe("afternoon");
    expect(greetingKey(21)).toBe("evening");
  });
});

describe("hub: Alt+Su e Alt+Giù", () => {
  test("pagina successiva e precedente, in giro", () => {
    const pages = ["general", "dictionary", "snippets"];
    expect(nextSection(pages, "general", 1)).toBe("dictionary");
    expect(nextSection(pages, "general", -1)).toBe("snippets");
    expect(nextSection(pages, "snippets", 1)).toBe("general");
  });
});
