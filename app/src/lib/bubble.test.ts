import { describe, expect, test } from "bun:test";
import {
  bubbleAnimates,
  bubbleView,
  isSphereView,
  transformRunningKey,
  createFrameLoop,
  dockLayout,
  type FrameScheduler,
} from "./bubble";
import { AQUILA, dotGrid, flapPose, wingLift } from "./eagle";

describe("disposizione della bolla secondo il bordo", () => {
  test("ogni bordo: bolla contro il lato, avviso verso il centro dello schermo", () => {
    expect(dockLayout("bottom")).toEqual({
      direction: "column",
      justify: "flex-end",
      align: "center",
      noticeSide: "above",
    });
    expect(dockLayout("top")).toEqual({
      direction: "column-reverse",
      justify: "flex-end",
      align: "center",
      noticeSide: "below",
    });
    expect(dockLayout("left")).toEqual({
      direction: "row-reverse",
      justify: "flex-end",
      align: "center",
      noticeSide: "right",
    });
    expect(dockLayout("right")).toEqual({
      direction: "row",
      justify: "flex-end",
      align: "center",
      noticeSide: "left",
    });
  });
  test("bordi laterali con la pillola larga: avviso sopra, attaccato al lato", () => {
    expect(dockLayout("left", true)).toMatchObject({ align: "flex-start", noticeSide: "above" });
    expect(dockLayout("right", true)).toMatchObject({ align: "flex-end", noticeSide: "above" });
    // In alto e in basso la pillola non cambia la disposizione.
    expect(dockLayout("bottom", true)).toEqual(dockLayout("bottom"));
  });
});

const fakeScheduler = () => {
  let next = 1;
  const pending = new Map<number, (t: number) => void>();
  const scheduler: FrameScheduler = {
    request: (cb) => {
      const id = next++;
      pending.set(id, cb);
      return id;
    },
    cancel: (id) => {
      pending.delete(id);
    },
  };
  const frame = (t: number) => {
    const cbs = [...pending.values()];
    pending.clear();
    cbs.forEach((cb) => cb(t));
  };
  return { scheduler, pending, frame };
};

describe("ciclo di animazione dei fili", () => {
  test("a riposo nessun ciclo: la sfera non chiede fotogrammi", () => {
    expect(bubbleView("idle", false)).toBe("sphere");
    expect(bubbleView(null, false)).toBe("sphere");
    expect(bubbleAnimates("sphere", false)).toBe(false);
    expect(bubbleAnimates("done", false)).toBe(false);
    // Con prefers-reduced-motion nemmeno la pillola anima.
    expect(bubbleAnimates("voice", true)).toBe(false);

    const { scheduler, pending } = fakeScheduler();
    let draws = 0;
    const loop = createFrameLoop(() => draws++, scheduler);
    loop.setActive(bubbleAnimates(bubbleView("idle", false), false));
    expect(pending.size).toBe(0);
    expect(loop.isRunning()).toBe(false);
    expect(draws).toBe(0);
  });
  test("parla: il ciclo parte; torna a riposo: nessuna richiesta in coda", () => {
    const { scheduler, pending, frame } = fakeScheduler();
    let draws = 0;
    const loop = createFrameLoop(() => draws++, scheduler);
    loop.setActive(bubbleAnimates(bubbleView("listening", false), false));
    expect(pending.size).toBe(1);
    frame(0);
    frame(16.7);
    expect(draws).toBe(2);
    loop.setActive(bubbleAnimates(bubbleView("idle", false), false));
    expect(pending.size).toBe(0);
    expect(loop.isRunning()).toBe(false);
  });
  test("al massimo 60 fotogrammi al secondo anche con uno schermo a 144 Hz", () => {
    const { scheduler, frame } = fakeScheduler();
    let draws = 0;
    const loop = createFrameLoop(() => draws++, scheduler);
    loop.setActive(true);
    for (let i = 0; i <= 144; i++) frame((i * 1000) / 144);
    expect(draws).toBeGreaterThanOrEqual(55);
    expect(draws).toBeLessThanOrEqual(61);
  });
});

describe("Orbita più Scia", () => {
  test("al rilascio sfera con le orbite; oltre 1 s pillola corta; all'incolla sfera con la spunta", () => {
    expect(bubbleView("transcribing", false, { workingMs: 0 })).toBe("orbit");
    expect(bubbleView("cleaning", false, { workingMs: 999 })).toBe("orbit");
    expect(bubbleView("pasting", false, { workingMs: 1000 })).toBe("linger");
    expect(bubbleView("pasted", false)).toBe("pasted");
    expect(isSphereView("orbit") && isSphereView("pasted")).toBe(true);
    expect(isSphereView("linger")).toBe(false);
  });
  test("orbite e cometa animano solo senza prefers-reduced-motion; l'incollato no", () => {
    expect(bubbleAnimates("orbit", false)).toBe(true);
    expect(bubbleAnimates("linger", false)).toBe(true);
    expect(bubbleAnimates("orbit", true)).toBe(false);
    expect(bubbleAnimates("pasted", false)).toBe(false);
    expect(bubbleAnimates("transformDone", false)).toBe(false);
  });
  test("Transform: Uso {nome} fino a 2 s, poi la frase lenta; finito diventa Vedi modifiche", () => {
    expect(bubbleView("idle", false, { transform: "running" })).toBe("transform");
    expect(bubbleView(null, false, { transform: "done" })).toBe("transformDone");
    expect(transformRunningKey(1999)).toBe("using");
    expect(transformRunningKey(2000)).toBe("slow");
    // La dettatura ha la precedenza sul Transform.
    expect(bubbleView("listening", false, { transform: "running" })).toBe("voice");
  });
});

describe("aquila a punti", () => {
  test("le pose delle ali cambiano i punti, il corpo resta", () => {
    const up = dotGrid(flapPose(wingLift(0)), 4, [0, -14, 100, 100]);
    const down = dotGrid(flapPose(wingLift(0.5)), 4, [0, -14, 100, 100]);
    expect(up.cells.length).toBeGreaterThan(80);
    expect(up.cells).not.toEqual(down.cells);
    // Il centro del corpo (50, 60) è pieno in tutte e due le pose.
    const body = (g: typeof up) => g.cells.some(([c, r]) => c === 12 && r === 18);
    expect(body(up) && body(down)).toBe(true);
    expect(flapPose(0)).toEqual(AQUILA.map(([x, y]) => [x, y]));
  });
});
