import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { ConnectScreen } from "./components/ConnectScreen";
import { DedicationScreen } from "./components/DedicationScreen";
import { SearchScreen } from "./components/SearchScreen";
import { SentScreen } from "./components/SentScreen";
import { type KioskConfig, loadConfig, saveConfig } from "./config";
import { ui } from "./text";
import type { ConnectionState, ErrorCode, RequestStatus, TrackInfo } from "./types";

/** Zdarzenia wysyłane przez warstwę Rust (nazwy muszą zgadzać się ze stałymi w `src-tauri`). */
const EVENT_CONNECTION = "kiosk://connection";
const EVENT_SEARCH_RESULTS = "kiosk://search-results";
const EVENT_REQUEST_STATUS = "kiosk://request-status";
const EVENT_ERROR = "kiosk://error";

type SearchResultsPayload = { request_id: number; tracks: TrackInfo[] };
type RequestStatusPayload = { request_id: number; status: RequestStatus };
type ErrorPayload = { request_id: number | null; code: ErrorCode };

type Screen = "search" | "dedication" | "sent";

/** Komunikat dla gościa. Kody błędów z sieci tłumaczymy tutaj — po sieci nie jadą teksty. */
function errorText(code: ErrorCode): string {
  return ui.errors[code] ?? ui.errors.unknown;
}

/**
 * Kiosk: gość szuka utworu, wybiera go i wpisuje dedykację. Kiosk niczym nie steruje —
 * wysyła prośbę i pokazuje jej status (PLAN.md, sekcja 4.2).
 */
export default function App() {
  const [connection, setConnection] = useState<ConnectionState>({ state: "disconnected" });
  const [config, setConfig] = useState<KioskConfig>(loadConfig);
  const [connecting, setConnecting] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const [screen, setScreen] = useState<Screen>("search");
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<TrackInfo[]>([]);
  const [searching, setSearching] = useState(false);
  const [hasSearched, setHasSearched] = useState(false);
  const [selected, setSelected] = useState<TrackInfo | null>(null);

  const [dedication, setDedication] = useState("");
  const [guestName, setGuestName] = useState("");
  const [sending, setSending] = useState(false);
  const [sentStatus, setSentStatus] = useState<RequestStatus>("submitted");

  // Identyfikatory trzymamy w referencjach: nasłuchy nie mogą się przepinać przy każdej zmianie.
  const searchRequestId = useRef<number | null>(null);
  const sentRequestId = useRef<number | null>(null);

  useEffect(() => {
    let active = true;

    invoke<ConnectionState>("connection_state")
      .then((state) => {
        if (active) {
          setConnection(state);
        }
      })
      .catch((reason: unknown) => {
        if (active) {
          setMessage(String(reason));
        }
      });

    const unlisten = listen<ConnectionState>(EVENT_CONNECTION, (event) => {
      setConnection(event.payload);
      setConnecting(false);

      if (event.payload.state === "connected") {
        setMessage(null);
      }
    });

    return () => {
      active = false;
      void unlisten.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    const unlistenResults = listen<SearchResultsPayload>(EVENT_SEARCH_RESULTS, (event) => {
      if (event.payload.request_id !== searchRequestId.current) {
        return;
      }

      setResults(event.payload.tracks);
      setSearching(false);
      setHasSearched(true);
    });

    const unlistenStatus = listen<RequestStatusPayload>(EVENT_REQUEST_STATUS, (event) => {
      if (event.payload.request_id !== sentRequestId.current) {
        return;
      }

      setSentStatus(event.payload.status);
      setSending(false);
      setScreen("sent");
    });

    const unlistenError = listen<ErrorPayload>(EVENT_ERROR, (event) => {
      setMessage(errorText(event.payload.code));
      setSearching(false);
      setSending(false);
    });

    return () => {
      void unlistenResults.then((stop) => stop());
      void unlistenStatus.then((stop) => stop());
      void unlistenError.then((stop) => stop());
    };
  }, []);

  async function handleConnect() {
    const port = Number(config.port);

    if (!Number.isInteger(port) || port < 1024 || port > 65535) {
      setMessage(ui.setup.portError);

      return;
    }

    setMessage(null);
    setConnecting(true);

    try {
      await invoke("connect", {
        config: {
          address: config.address,
          port,
          pin: config.pin,
          kiosk_name: config.kiosk_name,
        },
      });

      saveConfig(config);
    } catch (reason: unknown) {
      setConnecting(false);
      setMessage(String(reason));
    }
  }

  async function handleCancel() {
    await invoke("disconnect");
    setConnecting(false);
  }

  async function handleSearch() {
    setMessage(null);
    setResults([]);
    setSearching(true);

    try {
      const requestId = await invoke<number>("search", { query });
      searchRequestId.current = requestId;
    } catch (reason: unknown) {
      setSearching(false);
      setMessage(String(reason));
    }
  }

  async function handleSubmit() {
    if (selected === null) {
      return;
    }

    setMessage(null);
    setSending(true);

    try {
      const requestId = await invoke<number>("submit_request", {
        track_id: selected.id,
        dedication,
        guest_name: guestName.trim() === "" ? null : guestName,
      });
      sentRequestId.current = requestId;
    } catch (reason: unknown) {
      setSending(false);
      setMessage(String(reason));
    }
  }

  function handleAgain() {
    setScreen("search");
    setQuery("");
    setResults([]);
    setHasSearched(false);
    setSelected(null);
    setDedication("");
    setGuestName("");
    setMessage(null);
    sentRequestId.current = null;
  }

  if (connection.state !== "connected") {
    return (
      <div className="h-full bg-scena-950">
        <ConnectScreen
          config={config}
          connecting={connecting || connection.state === "connecting"}
          message={message}
          onChange={setConfig}
          onConnect={() => void handleConnect()}
          onCancel={() => void handleCancel()}
        />
      </div>
    );
  }

  return (
    <div className="h-full bg-scena-950">
      {screen === "search" && (
        <SearchScreen
          query={query}
          results={results}
          searching={searching}
          hasSearched={hasSearched}
          message={message}
          onQueryChange={setQuery}
          onSearch={() => void handleSearch()}
          onSelect={(track) => {
            setSelected(track);
            setMessage(null);
            setScreen("dedication");
          }}
        />
      )}

      {screen === "dedication" && selected !== null && (
        <DedicationScreen
          track={selected}
          dedication={dedication}
          guestName={guestName}
          limits={connection.limits}
          sending={sending}
          message={message}
          onDedicationChange={setDedication}
          onGuestNameChange={setGuestName}
          onSubmit={() => void handleSubmit()}
          onBack={() => setScreen("search")}
        />
      )}

      {screen === "sent" && selected !== null && (
        <SentScreen track={selected} status={sentStatus} onAgain={handleAgain} />
      )}
    </div>
  );
}
