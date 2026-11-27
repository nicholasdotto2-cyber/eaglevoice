// Backend finto per il browser (solo con VITE_MOCK_BACKEND=1): main.tsx importa
// questo modulo, quindi il mock è pronto prima delle prime chiamate a Tauri.
import "./mock-install";

const objectConstructor = Object as typeof Object & {
  hasOwn?: (target: object, key: PropertyKey) => boolean;
};

/** Install compatibility shims required by frontend dependencies. */
export const installCompatShims = (): void => {
  // react-markdown uses Object.hasOwn, which is unavailable before Safari 15.4.
  if (typeof objectConstructor.hasOwn !== "function") {
    Object.defineProperty(Object, "hasOwn", {
      value: (target: object, key: PropertyKey): boolean =>
        Object.prototype.hasOwnProperty.call(target, key),
      configurable: true,
      writable: true,
    });
  }
};
