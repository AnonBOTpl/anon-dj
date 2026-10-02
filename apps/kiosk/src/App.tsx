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
const EVENT_SUGGESTIONS = "kiosk://suggestions";
const EVENT_REQUEST_STATUS = "kiosk://request-status";
const EVENT_ERROR = "kiosk://error";

type SearchResultsPayload = { request_id: number; tracks: TrackInfo[] };
type RequestStatusPayload = { request_id: number; status: RequestStatus };
type ErrorPayload = { request_id: number | null; code: ErrorCode };

type Screen = "search" | "dedication" | "sent";

/** Zapasowy czas powrotu ekranu potwierdzenia, gdy aplikacja DJ-a go nie przysłała. */
const FALLBACK_CONFIRMATION_SECONDS = 20;

/**
 * Co ile najwyżej wysyłamy podpowiedzi. Bez tego każde wciśnięcie klawisza byłoby osobnym
 * zapytaniem do bazy DJ-a (PLAN.md, sekcja 11).
 */
const SUGGEST_DEBOUNCE_MS = 200;

/**
 * Od ilu znaków pytamy o podpowiedzi. Jedna litera pasuje do połowy biblioteki — lista byłaby
 * długa, wolna i niczego nie zawężała.
 */
const MIN_SUGGEST_QUERY_CHARS = 2;

/**
 * Co ile odświeżamy listę pod wpisaną frazą.
 *
 * Kiosk nie ma skąd wiedzieć, że DJ właśnie zeskanował bibliotekę — nie ma o tym żadnego
 * komunikatu w protokole. Fraza gościa się przy tym nie zmienia, więc bez tego odświeżenia
 * gość, który wpisał frazę w pustą bibliotekę, nie zobaczy wyników aż do następnego dotknięcia
 * klawiatury (zgłoszenie DJ-a: „kiosk nie dostaje aktualizacji”).
 *
 * Odpytanie jest tanie i mieści się w osobnym budżecie podpowiedzi (60 komunikatów na 10 s),
 * więc nie zjada limitu przeznaczonego na wyszukiwania i zgłoszenia gościa.
 */
const SEARCH_REFRESH_MS = 5_000;

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
  const [sentSecondsLeft, setSentSecondsLeft] = useState(FALLBACK_CONFIRMATION_SECONDS);
  // Licznik, który sam z siebie odświeża listę wyników — patrz [`SEARCH_REFRESH_MS`].
  const [refreshTick, setRefreshTick] = useState(0);

  // Identyfikatory trzymamy w referencjach: nasłuchy nie mogą się przepinać przy każdej zmianie.
  const searchRequestId = useRef<number | null>(null);
  const suggestRequestId = useRef<number | null>(null);
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

    // Nieaktualne podpowiedzi odrzucamy po identyfikatorze — inaczej odpowiedź na starszą frazę
    // podmieniłaby listę pod palcem gościa.
    const unlistenSuggestions = listen<SearchResultsPayload>(EVENT_SUGGESTIONS, (event) => {
      if (event.payload.request_id !== suggestRequestId.current) {
        return;
      }

      setResults(event.payload.tracks);
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
      void unlistenSuggestions.then((stop) => stop());
      void unlistenStatus.then((stop) => stop());
      void unlistenError.then((stop) => stop());
    };
  }, []);

  // Podpowiedzi na żywo: gość pisze, a lista sama się zawęża — nie musi klikać „Szukaj”
  // (PLAN.md, sekcja 11). Wysyłamy je dopiero po pauzie w pisaniu.
  useEffect(() => {
    if (connection.state !== "connected" || screen !== "search") {
      return;
    }

    const trimmed = query.trim();

    if (trimmed.length < MIN_SUGGEST_QUERY_CHARS) {
      // Za krótka fraza — wracamy do pustej listy zamiast pokazywać przypadkowe trafienia.
      setResults([]);
      setHasSearched(false);

      return;
    }

    let active = true;

    const timer = window.setTimeout(() => {
      // Porzucamy ewentualne wcześniejsze wyszukiwanie: jego odpowiedź nie może już podmienić
      // listy, bo gość od tego czasu dopisał literę.
      searchRequestId.current = null;

      void invoke<number>("suggest", { query: trimmed })
        .then((requestId) => {
          if (active) {
            suggestRequestId.current = requestId;
          }
        })
        .catch(() => {
          // Podpowiedzi to wygoda — brak odpowiedzi nie może pokazać gościowi błędu.
        });
    }, SUGGEST_DEBOUNCE_MS);

    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [query, screen, connection.state, refreshTick]);

  // Odświeżanie w tle: tyka tylko na ekranie szukania i tylko przy działającym połączeniu.
  useEffect(() => {
    if (connection.state !== "connected" || screen !== "search") {
      return;
    }

    const timer = window.setInterval(
      () => setRefreshTick((tick) => tick + 1),
      SEARCH_REFRESH_MS,
    );

    return () => window.clearInterval(timer);
  }, [screen, connection.state]);

  /* Czas powrotu ustawia DJ w swojej aplikacji — kiosk dostaje go przy parowaniu. */
  const confirmationSeconds =
    connection.state === "connected"
      ? connection.confirmation_seconds
      : FALLBACK_CONFIRMATION_SECONDS;

  // Po wysłaniu dedykacji kiosk sam wraca do szukania — kolejny gość nie musi nic klikać.
  useEffect(() => {
    if (screen !== "sent") {
      return;
    }

    setSentSecondsLeft(confirmationSeconds);

    const tick = window.setInterval(() => {
      setSentSecondsLeft((left) => Math.max(0, left - 1));
    }, 1000);
    const finish = window.setTimeout(handleAgain, confirmationSeconds * 1000);

    return () => {
      window.clearInterval(tick);
      window.clearTimeout(finish);
    };
  }, [screen, confirmationSeconds]);

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
    // Pełne wyszukiwanie unieważnia podpowiedzi, które jeszcze lecą — inaczej spóźniona
    // odpowiedź na starszą frazę nadpisałaby jego wyniki.
    suggestRequestId.current = null;

    try {
      const requestId = await invoke<number>("search", { query });
      searchRequestId.current = requestId;
    } catch (reason: unknown) {
      setSearching(false);
      setMessage(String(reason));
    }
  }

  function handleSelect(track: TrackInfo) {
    setSelected(track);
    setMessage(null);
    setScreen("dedication");
  }

  /// Enter wybiera pierwszą podpowiedź — gość nie ma celować kursorem w listę.
  /// Gdy podpowiedzi nie ma (za krótka fraza albo nic nie pasuje), Enter szuka jak dotąd.
  function handleEnter() {
    if (query.trim() === "") {
      return;
    }

    const first = results[0];

    if (first !== undefined) {
      handleSelect(first);

      return;
    }

    void handleSearch();
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
    suggestRequestId.current = null;
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
          maxQueryChars={connection.limits.search_query_max_chars}
          onQueryChange={setQuery}
          onSearch={() => void handleSearch()}
          onEnter={handleEnter}
          onSelect={handleSelect}
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
        <SentScreen
          track={selected}
          status={sentStatus}
          secondsLeft={sentSecondsLeft}
          onAgain={handleAgain}
        />
      )}
    </div>
  );
}
