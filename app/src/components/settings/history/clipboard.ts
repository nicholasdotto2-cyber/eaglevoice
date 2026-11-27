import { platform } from "@tauri-apps/plugin-os";
import { commands } from "@/bindings";

export type ClipboardWriter = Pick<Clipboard, "writeText">;

/** Backend writer: on Windows dictated text stays out of clipboard history and cloud sync. */
const protectedClipboard: ClipboardWriter = {
  writeText: async (text) => {
    const result = await commands.copyTextToClipboard(text);
    if (result.status === "error") throw new Error(result.error);
  },
};

const defaultClipboard = (): ClipboardWriter =>
  platform() === "windows" ? protectedClipboard : navigator.clipboard;

export const copyToClipboard = async (
  text: string,
  clipboard: ClipboardWriter = defaultClipboard(),
): Promise<boolean> => {
  try {
    await clipboard.writeText(text);
    return true;
  } catch (error) {
    console.error("Failed to copy to clipboard:", error);
    return false;
  }
};
