import type { MouseEvent } from "react";
import { Minus, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { ui } from "../text";

const appWindow = getCurrentWindow();

/**
 * Własny pasek tytułu okna bez ramki.
 *
 * Warstwa przeciągania (`data-tauri-drag-region`) jest osobnym divem pod spodem (z-0) —
 * nigdy na elemencie zawierającym przyciski, bo pożerałaby ich kliknięcia.
 *
 * Tauri sprawdza przy `mousedown` **cel zdarzenia**: element z atrybutem musi być tym klikniętym.
 * Warstwa z treścią (z-10) rozciąga się na cały nagłówek, więc bez `pointer-events-none`
 * zasłaniałaby warstwę przeciągania i okna nie dałoby się przesunąć. Przyciski dostają zdarzenia
 * z powrotem przez `pointer-events-auto` — dodatkowo Tauri sam pomija elementy klikalne
 * (przycisk, pole tekstowe), więc przeciąganie ich nie łapie.
 */
export function TitleBar() {
  async function handleMinimize(event: MouseEvent<HTMLButtonElement>) {
    event.stopPropagation();
    await appWindow.minimize();
  }

  async function handleClose(event: MouseEvent<HTMLButtonElement>) {
    event.stopPropagation();
    await appWindow.close();
  }

  return (
    <header className="relative flex h-9 shrink-0 items-center border-b border-scena-800 bg-scena-900 px-3">
      <div data-tauri-drag-region className="absolute inset-0 z-0" />

      <div className="pointer-events-none relative z-10 flex w-full items-center justify-between">
        <span className="text-xs font-semibold tracking-[0.3em] text-zinc-400">
          {ui.appName}
        </span>

        <div className="pointer-events-auto flex items-center gap-1">
          <button
            type="button"
            onClick={handleMinimize}
            title={ui.window.minimize}
            aria-label={ui.window.minimize}
            className="flex h-7 w-9 items-center justify-center rounded text-zinc-400 transition-colors hover:bg-scena-700 hover:text-zinc-100"
          >
            <Minus className="h-4 w-4" />
          </button>

          <button
            type="button"
            onClick={handleClose}
            title={ui.window.close}
            aria-label={ui.window.close}
            className="flex h-7 w-9 items-center justify-center rounded text-zinc-400 transition-colors hover:bg-red-600 hover:text-white"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      </div>
    </header>
  );
}
