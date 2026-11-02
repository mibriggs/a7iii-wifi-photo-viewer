// Calls into the Rust side (src-tauri/src/lib.rs).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Photo = {
  index: number;
  title: string;
  date: string;
  folder: string;
};

/** The download folder, and which photos (by index) already have a file in it. */
export type Folder = {
  downloadDir: string;
  saved: number[];
};

export type Session = Folder & {
  sessionId: number;
  cameraName: string;
  photos: Photo[];
};

export type Downloaded = { path: string; new: boolean };

export type DownloadStatus = "queued" | "downloading" | "saved" | "error";

/** Bytes received so far for one photo; `total` is null if the camera didn't send the size. */
export type DownloadProgress = { index: number; received: number; total: number | null };

/** Calls `onProgress` while downloads run. Returns a function that stops listening. */
export async function watchDownloads(onProgress: (p: DownloadProgress) => void): Promise<() => void> {
  return listen<DownloadProgress>("download-progress", (e) => onProgress(e.payload));
}

/** Calls `onProgress` while full-size originals load for zooming. */
export async function watchOriginals(onProgress: (p: DownloadProgress) => void): Promise<() => void> {
  return listen<DownloadProgress>("original-progress", (e) => onProgress(e.payload));
}

/** Fraction done (0–1), or null when the size isn't known. */
export function fractionDone(p: DownloadProgress | undefined): number | null {
  if (!p) return 0;
  return p.total ? Math.min(p.received / p.total, 1) : null;
}

export function formatMB(bytes: number): string {
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

export function downloadLabel(status: DownloadStatus | undefined, progress?: DownloadProgress): string {
  switch (status) {
    case "queued":
      return "Queued";
    case "downloading": {
      const fraction = fractionDone(progress);
      if (fraction !== null) return `Downloading ${Math.round(fraction * 100)}%`;
      return progress ? `Downloading ${formatMB(progress.received)}` : "Downloading…";
    }
    case "saved":
      return "Saved";
    case "error":
      return "Retry download";
    default:
      return "Download";
  }
}

const DOWNLOAD_DIR_KEY = "downloadDir";

/** The folder chosen in an earlier run, or "" for the default (Documents/SonyCamera). */
function savedDownloadDir(): string {
  try {
    return localStorage.getItem(DOWNLOAD_DIR_KEY) ?? "";
  } catch {
    return "";
  }
}

/** Connects and reads the card. `address` is blank to search the network. */
export async function connect(address: string, onProgress: (found: number) => void): Promise<Session> {
  const stopListening = await listen<number>("scan-progress", (e) => onProgress(e.payload));
  try {
    return await invoke<Session>("connect", { address, downloadDir: savedDownloadDir() });
  } finally {
    stopListening();
  }
}

export const disconnect = () => invoke<void>("disconnect");

/** Checks the download folder again for which photos have a file in it. */
export const checkSaved = () => invoke<Folder>("check_saved");

/** Changes (and remembers) the download folder. */
export async function setDownloadDir(path: string): Promise<Folder> {
  const folder = await invoke<Folder>("set_download_dir", { path });
  try {
    localStorage.setItem(DOWNLOAD_DIR_KEY, folder.downloadDir);
  } catch {}
  return folder;
}

/** Saves the full-size original into Documents/SonyCamera. */
export const download = (index: number) => invoke<Downloaded>("download", { index });

/** Address of a thumbnail ("tn"), preview ("lrg") or original ("org"), served by the Rust side. */
export function imageUrl(sessionId: number, size: "tn" | "lrg" | "org", index: number): string {
  const path = `${sessionId}/${size}/${index}`;
  // Windows' webview only accepts custom schemes in the http://<scheme>.localhost form.
  return navigator.userAgent.includes("Windows") ? `http://camera.localhost/${path}` : `camera://localhost/${path}`;
}

/** "2026-10-24" or "2026-10-24T11:37:11" → "Saturday 24 October 2026"; anything else as-is. */
export function formatDay(text: string): string {
  const day = new Date(`${text.slice(0, 10)}T12:00:00`);
  if (Number.isNaN(day.getTime())) return text || "Unknown date";
  return day.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" });
}

/** "2026-10-24T11:37:11" → "11:37"; blank if there's no time. */
export function formatTime(text: string): string {
  const match = text.match(/T(\d{2}:\d{2})/);
  return match ? match[1] : "";
}
