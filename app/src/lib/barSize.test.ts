import { describe, expect, test } from "bun:test";
import { createBarSizer } from "./barSize";

const fakeAdapter = () => {
  const calls: string[] = [];
  return {
    calls,
    adapter: {
      expand: () => calls.push("expand"),
      rest: () => calls.push("rest"),
    },
  };
};

describe("misura della barra con le notifiche", () => {
  test("notifica a riposo, expand, chiusa o copiata, rest", () => {
    const { calls, adapter } = fakeAdapter();
    const sizer = createBarSizer(adapter);
    expect(sizer.update({ openNotices: 0 })).toBeNull();
    // Arriva una notifica a riposo: una sola chiamata expand.
    expect(sizer.update({ openNotices: 1 })).toBe("expand");
    expect(sizer.update({ openNotices: 2 })).toBeNull();
    // Una si chiude ma l'altra resta: niente rest.
    expect(sizer.update({ openNotices: 1 })).toBeNull();
    // Chiusa (o copiata) l'ultima: rest.
    expect(sizer.update({ openNotices: 0 })).toBe("rest");
    expect(calls).toEqual(["expand", "rest"]);
  });
  test("incolla fallito: il backend apre la barra piena, la chiusura chiama solo rest", () => {
    const { calls, adapter } = fakeAdapter();
    const sizer = createBarSizer(adapter);
    sizer.markExpanded();
    expect(sizer.update({ openNotices: 1 })).toBeNull();
    expect(sizer.update({ openNotices: 0 })).toBe("rest");
    expect(calls).toEqual(["rest"]);
  });
  test("avviso durante la dettatura: expand; il backend rimette la misura, si richiede", () => {
    const { calls, adapter } = fakeAdapter();
    const sizer = createBarSizer(adapter);
    expect(sizer.update({ openNotices: 1 })).toBe("expand");
    // show-overlay "recording": la finestra torna 300x72, l'avviso è ancora aperto.
    sizer.markRested();
    expect(sizer.update({ openNotices: 1 })).toBe("expand");
    expect(calls).toEqual(["expand", "expand"]);
  });
});
