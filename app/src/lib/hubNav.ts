/** Navigazione dell'hub da tastiera (Alt+Su, Alt+Giù): logica pura, provata con `bun test`. */

/** Pagina successiva (`step` 1) o precedente (-1), in giro: dall'ultima si torna alla prima. */
export const nextSection = <T extends string>(pages: T[], current: T, step: 1 | -1): T => {
  if (pages.length === 0) return current;
  const i = pages.indexOf(current);
  if (i < 0) return pages[0];
  return pages[(i + step + pages.length) % pages.length];
};
