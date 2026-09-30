import { useCallback, useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { motion } from "framer-motion";
import {
  Disc3,
  Library,
  ListChecks,
  Server,
  Settings,
  TriangleAlert,
  Wifi,
  WifiOff,
} from "lucide-react";

import { LibraryView } from "./components/LibraryView";
import { Panel } from "./components/Panel";
import { RequestList, type QueuedRequest } from "./components/RequestList";
import { SettingsView } from "./components/SettingsView";
import { TitleBar } from "./components/TitleBar";
import { ui } from "./text";

/** Odpowiedź komendy `app_status` z warstwy Rust. */
type AppStatus = {
  version: string;
  protocol_version: number;
  db_path: string;
  log_dir: string;
};

/** Stan serwera kiosków — zgadza się z `ServerStatus` w warstwie Rust. */
type ServerStatus = {
  port: number;
  address: string | null;
  listening: boolean;
  connected: number;
  kiosks: string[];
};

/** Zdarzenie o zmianie kolejki prośb. */
const EVENT_REQUESTS_CHANGED = "requests://changed";
/** Zdarzenie o zmianie stanu serwera kiosków. */
const EVENT_KIOSK_STATUS = "kiosk://status";

type View = "queues" | "library" | "settings";

type ChipTone = "neutral" | "warning" | "danger";

const chipTones: Record<ChipTone, string> = {
  neutral: "border-scena-700 text-zinc-300",
  warning: "border-amber-700/60 text-amber-300",
  danger: "border-red-700/60 text-red-300",
};

function Chip({
  icon,
  label,
  value,
  tone,
}: {
  icon: ReactNode;
  label: string;
  value: string;
  tone: ChipTone;
}) {
  return (
    <motion.span
      layout
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ duration: 0.15 }}
      className={`flex min-w-0 items-center gap-2 rounded-full border bg-scena-900 px-3 py-1 text-xs ${chipTones[tone]}`}
    >
      {icon}
      <span className="text-zinc-500">{label}</span>
      <span className="truncate font-medium">{value}</span>
    </motion.span>
  );
}

function NavButton({
  active,
  icon,
  label,
  onClick,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`flex items-center gap-2 rounded px-3 py-1 text-xs transition-colors ${
        active
          ? "bg-scena-700 text-zinc-100"
          : "text-zinc-400 hover:bg-scena-800 hover:text-zinc-200"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}

export default function App() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [server, setServer] = useState<ServerStatus | null>(null);
  const [requests, setRequests] = useState<QueuedRequest[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<View>("queues");

  const refreshRequests = useCallback(async () => {
    const pending = await invoke<QueuedRequest[]>("pending_requests");

    setRequests(pending);
  }, []);

  // Serwer kiosków melduje się zdarzeniem; na starcie pytamy o stan raz, żeby pasek statusu
  // nie czekał na pierwsze połączenie.
  useEffect(() => {
    let active = true;

    invoke<ServerStatus>("server_status")
      .then((value) => {
        if (active) {
          setServer(value);
        }
      })
      .catch((reason: unknown) => {
        if (active) {
          setError(String(reason));
        }
      });

    const unlisten = listen<ServerStatus>(EVENT_KIOSK_STATUS, (event) => {
      setServer(event.payload);
    });

    return () => {
      active = false;
      void unlisten.then((stop) => stop());
    };
  }, []);

  // Kolejka prośb: raz na start i po każdym zgłoszeniu gościa.
  useEffect(() => {
    void refreshRequests().catch((reason: unknown) => {
      setError(`${ui.requests.loadError} ${String(reason)}`);
    });

    const unlisten = listen(EVENT_REQUESTS_CHANGED, () => {
      void refreshRequests().catch((reason: unknown) => {
        setError(`${ui.requests.loadError} ${String(reason)}`);
      });
    });

    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [refreshRequests]);

  useEffect(() => {
    let active = true;

    invoke<AppStatus>("app_status")
      .then((value) => {
        if (active) {
          setStatus(value);
        }
      })
      .catch((reason: unknown) => {
        if (active) {
          setError(String(reason));
        }
      });

    return () => {
      active = false;
    };
  }, []);

  const kioskConnected = (server?.connected ?? 0) > 0;
  const kioskValue = kioskConnected
    ? (server?.kiosks ?? []).join(", ")
    : ui.status.kioskDisconnected;

  const serverValue =
    server === null
      ? ui.status.loading
      : server.listening
        ? `${server.address ?? ui.status.serverUnknownAddress}:${server.port}`
        : ui.status.serverUnavailable;

  return (
    <div className="app-chrome flex h-full flex-col bg-scena-950">
      <TitleBar />

      <section className="flex shrink-0 items-center gap-2 border-b border-scena-800 bg-scena-900/60 px-3 py-2">
        <Chip
          icon={<Disc3 className="h-3.5 w-3.5" />}
          label={ui.status.player}
          value={ui.status.playerUnknown}
          tone="neutral"
        />
        <Chip
          icon={kioskConnected ? <Wifi className="h-3.5 w-3.5" /> : <WifiOff className="h-3.5 w-3.5" />}
          label={ui.status.kiosk}
          value={kioskValue}
          tone={kioskConnected ? "neutral" : "warning"}
        />
        <Chip
          icon={<Server className="h-3.5 w-3.5" />}
          label={ui.status.server}
          value={serverValue}
          tone={server === null || server.listening ? "neutral" : "danger"}
        />
        {error !== null && (
          <Chip
            icon={<TriangleAlert className="h-3.5 w-3.5" />}
            label={ui.status.errorPrefix}
            value={error}
            tone="danger"
          />
        )}

        <nav className="ml-auto flex items-center gap-1">
          <NavButton
            active={view === "queues"}
            icon={<ListChecks className="h-4 w-4" />}
            label={ui.nav.queues}
            onClick={() => setView("queues")}
          />
          <NavButton
            active={view === "library"}
            icon={<Library className="h-4 w-4" />}
            label={ui.nav.library}
            onClick={() => setView("library")}
          />
          <NavButton
            active={view === "settings"}
            icon={<Settings className="h-4 w-4" />}
            label={ui.nav.settings}
            onClick={() => setView("settings")}
          />
        </nav>
      </section>

      <main className="min-h-0 flex-1 overflow-y-auto p-3">
        {view === "queues" && (
          <div className="grid h-full min-h-0 grid-cols-[1.5fr_1fr] gap-3">
            <Panel
              title={ui.queue.reviewTitle}
              hint={ui.queue.reviewHint}
              empty={ui.queue.reviewEmpty}
            >
              {requests.length > 0 ? <RequestList requests={requests} /> : undefined}
            </Panel>

            <div className="grid min-h-0 grid-rows-2 gap-3">
              <Panel
                title={ui.queue.readyTitle}
                hint={ui.queue.readyHint}
                empty={ui.queue.readyEmpty}
              />
              <Panel
                title={ui.queue.historyTitle}
                hint={ui.queue.historyHint}
                empty={ui.queue.historyEmpty}
              />
            </div>
          </div>
        )}

        {view === "library" && <LibraryView />}

        {view === "settings" && <SettingsView />}
      </main>

      <footer className="flex shrink-0 items-center gap-4 border-t border-scena-800 bg-scena-900/60 px-3 py-1.5 text-[11px] text-zinc-500">
        {status !== null ? (
          <>
            <span>
              {ui.footer.version} {status.version}
            </span>
            <span>
              {ui.footer.protocol} {status.protocol_version}
            </span>
            <span className="truncate" title={status.db_path}>
              {ui.footer.db}: {status.db_path}
            </span>
            <span className="truncate" title={status.log_dir}>
              {ui.footer.logs}: {status.log_dir}
            </span>
          </>
        ) : (
          <span>{ui.status.loading}</span>
        )}
      </footer>
    </div>
  );
}
