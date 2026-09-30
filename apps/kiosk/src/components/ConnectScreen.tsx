import { motion } from "framer-motion";
import { LoaderCircle, Plug } from "lucide-react";

import type { KioskConfig } from "../config";
import { ui } from "../text";

type ConnectScreenProps = {
  config: KioskConfig;
  connecting: boolean;
  message: string | null;
  onChange: (config: KioskConfig) => void;
  onConnect: () => void;
  onCancel: () => void;
};

const inputClass =
  "h-14 w-full rounded-2xl border border-scena-700 bg-scena-950 px-5 text-lg text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-zinc-500";

/**
 * Ekran konfiguracji — widzi go wyłącznie DJ, i tylko dopóki kiosk nie jest połączony.
 * Gość nie powinien trafić na ten ekran w trakcie imprezy, dlatego pokazujemy go tylko wtedy,
 * gdy połączenia nie ma.
 */
export function ConnectScreen({
  config,
  connecting,
  message,
  onChange,
  onConnect,
  onCancel,
}: ConnectScreenProps) {
  function update(key: keyof KioskConfig, value: string) {
    onChange({ ...config, [key]: value });
  }

  return (
    <div className="flex h-full items-center justify-center px-16">
      <motion.section
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.25, ease: "easeOut" }}
        className="w-full max-w-3xl rounded-3xl border border-scena-800 bg-scena-900 p-10 shadow-2xl"
      >
        <h1 className="text-3xl font-semibold text-zinc-100">{ui.setup.heading}</h1>
        <p className="mt-2 text-lg text-zinc-400">{ui.setup.hint}</p>

        <div className="mt-8 grid grid-cols-2 gap-5">
          <label className="flex flex-col gap-2">
            <span className="text-sm font-medium text-zinc-300">{ui.setup.addressLabel}</span>
            <input
              type="text"
              value={config.address}
              placeholder={ui.setup.addressPlaceholder}
              onChange={(event) => update("address", event.currentTarget.value)}
              className={inputClass}
            />
          </label>

          <label className="flex flex-col gap-2">
            <span className="text-sm font-medium text-zinc-300">{ui.setup.portLabel}</span>
            <input
              type="number"
              min={1024}
              max={65535}
              value={config.port}
              onChange={(event) => update("port", event.currentTarget.value)}
              className={inputClass}
            />
          </label>

          <label className="flex flex-col gap-2">
            <span className="text-sm font-medium text-zinc-300">{ui.setup.pinLabel}</span>
            <input
              type="text"
              inputMode="numeric"
              maxLength={6}
              value={config.pin}
              onChange={(event) => update("pin", event.currentTarget.value)}
              className={`${inputClass} font-mono tracking-[0.3em]`}
            />
          </label>

          <label className="flex flex-col gap-2">
            <span className="text-sm font-medium text-zinc-300">{ui.setup.nameLabel}</span>
            <input
              type="text"
              maxLength={40}
              value={config.kiosk_name}
              onChange={(event) => update("kiosk_name", event.currentTarget.value)}
              className={inputClass}
            />
          </label>
        </div>

        <div className="mt-8 flex items-center gap-4">
          <button
            type="button"
            onClick={onConnect}
            disabled={connecting}
            className="flex h-14 items-center gap-3 rounded-2xl bg-zinc-100 px-8 text-lg font-semibold text-scena-950 transition-colors hover:bg-white disabled:opacity-50"
          >
            {connecting ? (
              <LoaderCircle className="h-6 w-6 animate-spin" />
            ) : (
              <Plug className="h-6 w-6" />
            )}
            {connecting ? ui.setup.connecting : ui.setup.connect}
          </button>

          {connecting && (
            <button
              type="button"
              onClick={onCancel}
              className="h-14 rounded-2xl border border-scena-700 px-8 text-lg text-zinc-300 transition-colors hover:bg-scena-800"
            >
              {ui.setup.cancel}
            </button>
          )}

          {message !== null && <p className="text-lg text-red-400">{message}</p>}
        </div>
      </motion.section>
    </div>
  );
}
