/**
 * Wszystkie teksty interfejsu DJ-a. Żaden tekst nie jest wpisany na sztywno w komponencie
 * (AGENTS.md, konwencje frontendu). Aplikacja jest po polsku.
 */
export const ui = {
  appName: "ANON DJ",
  window: {
    minimize: "Zminimalizuj",
    close: "Zamknij",
  },
  status: {
    player: "Odtwarzacz",
    playerUnknown: "nie sprawdzono",
    kiosk: "Kiosk",
    kioskDisconnected: "rozłączony",
    loading: "Wczytywanie…",
    errorPrefix: "Błąd",
  },
  queue: {
    reviewTitle: "Do przeglądu",
    reviewHint: "Dedykacje od gości czekają tutaj na Twoją decyzję.",
    reviewEmpty: "Brak dedykacji czekających na przegląd.",
    readyTitle: "Gotowe do wykonania",
    readyHint: "Zatwierdzone dedykacje z gotowym głosem.",
    readyEmpty: "Nic nie czeka na wykonanie.",
    historyTitle: "Historia",
    historyHint: "Wykonane i odrzucone dedykacje.",
    historyEmpty: "Historia jest pusta.",
  },
  nav: {
    queues: "Kolejki",
    settings: "Ustawienia",
  },
  settings: {
    title: "Ustawienia",
    hint: "Wszystkie wartości zapisują się w bazie i obowiązują od razu po zapisaniu.",
    pinLabel: "PIN parowania kiosku",
    pinHint: "Dokładnie sześć cyfr. Kiosk podaje ten kod przy łączeniu.",
    dedicationLimitLabel: "Limit dedykacji (znaki)",
    dedicationLimitHint: "Ile znaków może wpisać gość. Ten sam limit dostaje kiosk przy parowaniu.",
    guestNameLimitLabel: "Limit imienia gościa (znaki)",
    guestNameLimitHint: "Zero wyłącza pole imienia.",
    searchQueryLimitLabel: "Limit wyszukiwania (znaki)",
    save: "Zapisz",
    saving: "Zapisywanie…",
    saved: "Zapisano",
    loadError: "Nie udało się wczytać ustawień.",
    numberError: "Podaj poprawną liczbę.",
  },
  footer: {
    version: "Wersja",
    protocol: "Protokół",
    db: "Baza",
    logs: "Logi",
  },
} as const;
