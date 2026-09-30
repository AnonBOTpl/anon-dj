/**
 * Wszystkie teksty ekranu kiosku. Kiosk pokazuje gościowi jak najmniej informacji — żadnych
 * adresów, logów ani kodów błędów (AGENTS.md, konwencje frontendu).
 */
export const ui = {
  appName: "ANON DJ",

  // Ekran konfiguracji — widzi go wyłącznie DJ, przed imprezą.
  setup: {
    heading: "Konfiguracja kiosku",
    hint: "Ustaw raz przed imprezą. Adres i port znajdziesz w aplikacji DJ-a w pasku statusu.",
    addressLabel: "Adres komputera DJ-a",
    addressPlaceholder: "192.168.0.10",
    portLabel: "Port",
    pinLabel: "PIN parowania",
    nameLabel: "Nazwa tego kiosku",
    connect: "Połącz",
    connecting: "Łączę…",
    cancel: "Przerwij",
    portError: "Port musi być liczbą z zakresu 1024–65535.",
  },

  // Ekran wyszukiwania dla gościa.
  search: {
    heading: "Znajdź swoją piosenkę",
    subheading: "Wpisz tytuł albo wykonawcę.",
    placeholder: "Tytuł lub wykonawca…",
    button: "Szukaj",
    searching: "Szukam…",
    empty: "Nic nie znaleźliśmy. Spróbuj wpisać inaczej.",
    noArtist: "Nieznany wykonawca",
    disconnected: "Kiosk czeka na połączenie z komputerem DJ-a.",
  },

  // Ekran dedykacji po wybraniu utworu.
  dedication: {
    heading: "Twoja dedykacja",
    subheading: "Napisz, co DJ ma przeczytać przed piosenką.",
    placeholder: "Na przykład: Dla Kasi i Marka — sto lat!",
    guestNameLabel: "Twoje imię (możesz zostawić puste)",
    guestNamePlaceholder: "Ania",
    submit: "Wyślij dedykację",
    sending: "Wysyłam…",
    back: "Wróć do szukania",
    counterLabel: "znaków",
  },

  // Ekran potwierdzenia.
  sent: {
    heading: "Dedykacja poszła do DJ-a",
    subheading: "Zaraz zobaczy ją na swoim komputerze.",
    again: "Nowa dedykacja",
    // Odliczanie do samoczynnego powrotu do wyszukiwania — kolejny gość nie musi nic klikać.
    countdown: (seconds: number) => `Za ${seconds} s kiosk wróci do szukania.`,
  },

  // Statusy prośby pokazywane gościowi.
  status: {
    submitted: "Dedykacja czeka u DJ-a.",
    approved: "DJ zatwierdził Twoją dedykację.",
    playing: "Twoja dedykacja leci na parkiecie!",
    done: "Zagrane. Dziękujemy!",
    rejected: "DJ nie puścił tym razem tej dedykacji.",
  },

  // Komunikaty błędów — zawsze po ludzku, bez kodów technicznych.
  errors: {
    unauthorized: "Kiosk nie jest podłączony do komputera DJ-a.",
    invalid_message: "Coś poszło nie tak. Spróbuj jeszcze raz.",
    invalid_request: "Nie udało się przyjąć zgłoszenia. Spróbuj jeszcze raz.",
    too_long: "Tekst jest za długi.",
    rate_limited: "Za dużo zgłoszeń naraz — poczekaj chwilę.",
    duplicate_request: "Ta dedykacja już do nas dotarła.",
    internal: "Komputer DJ-a ma teraz problem. Spróbuj jeszcze raz.",
    unknown: "Nie udało się wysłać zgłoszenia. Spróbuj jeszcze raz.",
  },
} as const;
