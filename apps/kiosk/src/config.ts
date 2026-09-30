/**
 * Konfiguracja połączenia z aplikacją DJ-a.
 *
 * Trzymamy ją w `localStorage` webview, żeby DJ wpisał adres i PIN tylko raz, przed imprezą.
 * Kiosk nie prowadzi własnej bazy (PLAN.md, sekcja 4.2) — to jedyny trwały zapis po tej stronie,
 * a i tak nie zawiera niczego wrażliwego poza PIN-em do parowania.
 */
export type KioskConfig = {
  /** Adres komputera DJ-a w sieci lokalnej, np. `192.168.0.10`. */
  address: string;
  /** Port wpisany jako tekst — tak wygodniej trzymać pole formularza. */
  port: string;
  pin: string;
  kiosk_name: string;
};

const STORAGE_KEY = "anon-dj.kiosk.config";

/** Port domyślny — ten sam, od którego startuje świeża instalacja aplikacji DJ-a. */
export const DEFAULT_PORT = "8790";

/** Nazwa kiosku widoczna u DJ-a, dopóki ktoś nie wpisze własnej. */
export const DEFAULT_KIOSK_NAME = "Kiosk przy wejściu";

export const emptyConfig: KioskConfig = {
  address: "",
  port: DEFAULT_PORT,
  pin: "",
  kiosk_name: DEFAULT_KIOSK_NAME,
};

/** Wczytuje konfigurację. Każde pole sprawdzamy osobno — uszkodzony zapis nie może wywalić kiosku. */
export function loadConfig(): KioskConfig {
  let raw: string | null = null;

  try {
    raw = window.localStorage.getItem(STORAGE_KEY);
  } catch {
    return emptyConfig;
  }

  if (raw === null) {
    return emptyConfig;
  }

  try {
    const parsed: unknown = JSON.parse(raw);

    if (typeof parsed !== "object" || parsed === null) {
      return emptyConfig;
    }

    const record = parsed as Record<string, unknown>;
    const readText = (key: string, fallback: string): string => {
      const value = record[key];

      return typeof value === "string" ? value : fallback;
    };

    return {
      address: readText("address", emptyConfig.address),
      port: readText("port", emptyConfig.port),
      pin: readText("pin", emptyConfig.pin),
      kiosk_name: readText("kiosk_name", emptyConfig.kiosk_name),
    };
  } catch {
    return emptyConfig;
  }
}

/** Zapisuje konfigurację. Brak miejsca albo zablokowany magazyn nie może przerwać pracy kiosku. */
export function saveConfig(config: KioskConfig): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(config));
  } catch {
    // Kiosk będzie działał dalej — po restarcie trzeba będzie wpisać dane ponownie.
  }
}
