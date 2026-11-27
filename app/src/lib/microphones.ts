/**
 * Microfoni in ordine di preferenza: logica pura della lista in
 * Impostazioni, provata con `bun test`.
 */

export interface RankedMicrophone {
  name: string;
  /** True se il dispositivo è collegato adesso. */
  present: boolean;
}

/** Nome che Handy usa per "il microfono predefinito di Windows": non entra nella classifica. */
export const DEFAULT_MIC_NAME = "Default";

/**
 * Lista da mostrare: prima i nomi già in `ranked` (nel loro ordine, anche se
 * scollegati), poi i dispositivi presenti che non sono ancora in classifica.
 * Nomi ripetuti e "Default" vengono tolti.
 */
export const mergeRankedMicrophones = (
  ranked: readonly string[],
  present: readonly string[],
): RankedMicrophone[] => {
  const presentSet = new Set(present);
  const seen = new Set<string>();
  const out: RankedMicrophone[] = [];
  for (const name of [...ranked, ...present]) {
    if (!name || name === DEFAULT_MIC_NAME || seen.has(name)) continue;
    seen.add(name);
    out.push({ name, present: presentSet.has(name) });
  }
  return out;
};

/** Sposta la voce `index` di `delta` posizioni (-1 su, +1 giù). Fuori dai limiti non cambia nulla. */
export const moveItem = <T>(list: readonly T[], index: number, delta: number): T[] => {
  const target = index + delta;
  if (index < 0 || index >= list.length || target < 0 || target >= list.length) {
    return [...list];
  }
  const next = [...list];
  const [item] = next.splice(index, 1);
  next.splice(target, 0, item);
  return next;
};
