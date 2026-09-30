import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { motion } from "framer-motion";
import { FolderPlus, LoaderCircle, RefreshCw, Search, Trash } from "lucide-react";

import { ui } from "../text";

/**
 * Nazwa zdarzenia z postępem skanowania — musi zgadzać się ze stałą `EVENT_SCAN_PROGRESS`
 * w warstwie Rust.
 */
const SCAN_PROGRESS_EVENT = "library://scan-progress";

/** Opóźnienie wyszukiwania: baza nie dostaje zapytania na każdy wciśnięty znak. */
const SEARCH_DEBOUNCE_MS = 200;

/** Folder biblioteki — zgadza się z typem `LibraryFolder` w warstwie Rust. */
type LibraryFolder = {
  id: number;
  path: string;
};

/** Liczby biblioteki — zgadzają się z typem `LibraryStats` w warstwie Rust. */
type LibraryStats = {
  folders: number;
  tracks: number;
  scanning: boolean;
};

/** Utwór z biblioteki — ten sam kształt co `TrackInfo` w crate `protocol`. */
type LibraryTrack = {
  id: number;
  title: string;
  artist: string;
  album?: string | null;
  duration_ms?: number | null;
};

/** Postęp skanowania wysyłany z warstwy Rust. */
type ScanProgress = {
  folder: string;
  folders_done: number;
  folders_total: number;
  files_seen: number;
  indexed: number;
  skipped: number;
  done: boolean;
};

/** Czas trwania w postaci `m:ss` — w bibliotece liczy się zwięzłość. */
function formatDuration(durationMs?: number | null): string {
  if (durationMs === undefined || durationMs === null) {
    return ui.library.noDuration;
  }

  const totalSeconds = Math.round(durationMs / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;

  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/**
 * Widok biblioteki: foldery do skanowania, uruchomienie skanowania i wyszukiwanie
 * po tytule oraz wykonawcy (PLAN.md, Faza 2).
 */
export function LibraryView() {
  const [folders, setFolders] = useState<LibraryFolder[]>([]);
  const [stats, setStats] = useState<LibraryStats | null>(null);
  const [tracks, setTracks] = useState<LibraryTrack[]>([]);
  const [query, setQuery] = useState("");
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [searchNonce, setSearchNonce] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const reloadStats = useCallback(async () => {
    const libraryStats = await invoke<LibraryStats>("library_stats");

    setStats(libraryStats);
  }, []);

  // Wczytanie folderów i liczb przy wejściu w widok.
  useEffect(() => {
    let active = true;

    void (async () => {
      try {
        const [folderList, libraryStats] = await Promise.all([
          invoke<LibraryFolder[]>("library_folders"),
          invoke<LibraryStats>("library_stats"),
        ]);

        if (!active) {
          return;
        }

        setFolders(folderList);
        setStats(libraryStats);
      } catch (reason: unknown) {
        if (active) {
          setError(`${ui.library.loadError} ${String(reason)}`);
        }
      }
    })();

    return () => {
      active = false;
    };
  }, []);

  // Wyszukiwanie z opóźnieniem. `searchNonce` pozwala odświeżyć listę po skanie.
  useEffect(() => {
    let active = true;

    const timer = window.setTimeout(() => {
      void invoke<LibraryTrack[]>("search_tracks", { query })
        .then((found) => {
          if (active) {
            setTracks(found);
          }
        })
        .catch((reason: unknown) => {
          if (active) {
            setError(`${ui.library.searchError} ${String(reason)}`);
          }
        });
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [query, searchNonce]);

  // Postęp skanowania przychodzi zdarzeniami z warstwy Rust.
  useEffect(() => {
    const unlisten = listen<ScanProgress>(SCAN_PROGRESS_EVENT, (event) => {
      setProgress(event.payload);
    });

    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  // Po zakończeniu skanowania odświeżamy liczby i listę utworów.
  useEffect(() => {
    if (progress?.done !== true) {
      return;
    }

    setBusy(false);

    void (async () => {
      try {
        await reloadStats();
        setSearchNonce((current) => current + 1);
      } catch (reason: unknown) {
        setError(`${ui.library.loadError} ${String(reason)}`);
      }
    })();
  }, [progress, reloadStats]);

  async function handleAddFolder() {
    setError(null);

    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: ui.library.addFolder,
      });

      // `null` oznacza anulowanie okna — to nie jest błąd.
      if (typeof selected !== "string") {
        return;
      }

      const folderList = await invoke<LibraryFolder[]>("add_library_folder", {
        path: selected,
      });

      setFolders(folderList);
      await reloadStats();
    } catch (reason: unknown) {
      setError(`${ui.library.addError} ${String(reason)}`);
    }
  }

  async function handleRemoveFolder(id: number) {
    setError(null);

    try {
      const folderList = await invoke<LibraryFolder[]>("remove_library_folder", { id });

      setFolders(folderList);
      await reloadStats();
      setSearchNonce((current) => current + 1);
    } catch (reason: unknown) {
      setError(`${ui.library.removeError} ${String(reason)}`);
    }
  }

  async function handleScan() {
    setError(null);
    setBusy(true);
    setProgress(null);

    try {
      await invoke("start_library_scan");
    } catch (reason: unknown) {
      setBusy(false);
      setError(`${ui.library.scanError} ${String(reason)}`);
    }
  }

  const scanning = busy || stats?.scanning === true;
  const libraryIsEmpty = stats !== null && stats.tracks === 0;

  return (
    <div className="grid h-full min-h-0 grid-cols-[22rem_1fr] gap-3">
      <motion.section
        initial={{ opacity: 0, y: 6 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.18, ease: "easeOut" }}
        className="flex min-h-0 flex-col rounded-lg border border-scena-800 bg-scena-900"
      >
        <header className="border-b border-scena-800 px-3 py-2">
          <h2 className="text-sm font-semibold text-zinc-200">{ui.library.foldersTitle}</h2>
          <p className="text-xs text-zinc-500">{ui.library.foldersHint}</p>
        </header>

        <div className="min-h-0 flex-1 space-y-1.5 overflow-y-auto p-3">
          {folders.length === 0 ? (
            <p className="text-sm text-zinc-500">{ui.library.foldersEmpty}</p>
          ) : (
            folders.map((folder) => (
              <div
                key={folder.id}
                className="flex items-center gap-2 rounded border border-scena-800 bg-scena-950 px-2 py-1.5"
              >
                <span
                  className="min-w-0 flex-1 truncate font-mono text-xs text-zinc-300"
                  title={folder.path}
                >
                  {folder.path}
                </span>

                <button
                  type="button"
                  onClick={() => void handleRemoveFolder(folder.id)}
                  disabled={scanning}
                  title={ui.library.removeFolder}
                  aria-label={ui.library.removeFolder}
                  className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-zinc-500 transition-colors hover:bg-red-600 hover:text-white disabled:opacity-40"
                >
                  <Trash className="h-3.5 w-3.5" />
                </button>
              </div>
            ))
          )}
        </div>

        <footer className="space-y-2 border-t border-scena-800 p-3">
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleAddFolder()}
              disabled={scanning}
              className="flex items-center gap-1.5 rounded border border-scena-700 px-2.5 py-1.5 text-xs text-zinc-300 transition-colors hover:bg-scena-800 disabled:opacity-40"
            >
              <FolderPlus className="h-3.5 w-3.5" />
              {ui.library.addFolder}
            </button>

            <button
              type="button"
              onClick={() => void handleScan()}
              disabled={scanning || folders.length === 0}
              className="flex items-center gap-1.5 rounded bg-zinc-100 px-2.5 py-1.5 text-xs font-semibold text-scena-950 transition-colors hover:bg-white disabled:opacity-40"
            >
              {scanning ? (
                <LoaderCircle className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <RefreshCw className="h-3.5 w-3.5" />
              )}
              {scanning ? ui.library.scanRunning : ui.library.scan}
            </button>
          </div>

          <p className="text-xs text-zinc-500">
            {ui.library.statsFolders}: {stats?.folders ?? 0} · {ui.library.statsTracks}:{" "}
            {stats?.tracks ?? 0}
          </p>

          {progress !== null && (
            <p className="truncate text-xs text-zinc-400" title={progress.folder}>
              {ui.library.progressFolder} {progress.folders_done}/{progress.folders_total}:{" "}
              {progress.files_seen} {ui.library.progressFiles}, {progress.indexed}{" "}
              {ui.library.progressIndexed}, {progress.skipped} {ui.library.progressSkipped}
            </p>
          )}

          {error !== null && <p className="text-xs text-red-400">{error}</p>}
        </footer>
      </motion.section>

      <motion.section
        initial={{ opacity: 0, y: 6 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.18, ease: "easeOut" }}
        className="flex min-h-0 flex-col rounded-lg border border-scena-800 bg-scena-900"
      >
        <header className="border-b border-scena-800 px-3 py-2">
          <label className="flex items-center gap-2">
            <Search className="h-4 w-4 shrink-0 text-zinc-500" />
            <span className="text-sm font-semibold text-zinc-200">
              {ui.library.searchLabel}
            </span>
            <input
              type="text"
              value={query}
              placeholder={ui.library.searchPlaceholder}
              onChange={(event) => setQuery(event.currentTarget.value)}
              className="ml-auto w-72 rounded border border-scena-700 bg-scena-950 px-3 py-1.5 text-sm text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-zinc-500"
            />
          </label>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto">
          {tracks.length === 0 ? (
            <p className="p-3 text-sm text-zinc-500">
              {libraryIsEmpty ? ui.library.libraryEmpty : ui.library.searchEmpty}
            </p>
          ) : (
            tracks.map((track) => (
              <div
                key={track.id}
                className="flex items-baseline gap-3 border-b border-scena-800/60 px-3 py-1.5 last:border-b-0"
              >
                <span
                  className="min-w-0 flex-1 truncate text-sm text-zinc-100"
                  title={track.title}
                >
                  {track.title}
                </span>
                <span
                  className="w-44 shrink-0 truncate text-xs text-zinc-400"
                  title={track.artist}
                >
                  {track.artist === "" ? ui.library.unknownArtist : track.artist}
                </span>
                <span
                  className="w-36 shrink-0 truncate text-xs text-zinc-500"
                  title={track.album ?? ""}
                >
                  {track.album ?? ui.library.unknownAlbum}
                </span>
                <span className="w-10 shrink-0 text-right text-xs tabular-nums text-zinc-500">
                  {formatDuration(track.duration_ms)}
                </span>
              </div>
            ))
          )}
        </div>

        <footer className="border-t border-scena-800 px-3 py-1.5 text-[11px] text-zinc-500">
          {ui.library.hint}
        </footer>
      </motion.section>
    </div>
  );
}
