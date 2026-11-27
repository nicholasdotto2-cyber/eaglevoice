/**
 * Installa il backend finto prima di qualunque chiamata a Tauri, solo con
 * `VITE_MOCK_BACKEND=1`. Nel build normale la condizione è falsa a tempo di
 * compilazione e il mock resta fuori dal bundle.
 */
import { installMockBackend } from "./mock-backend";

if (import.meta.env.VITE_MOCK_BACKEND === "1") {
  installMockBackend();
}
