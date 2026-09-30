# ANON DJ

Lokalna aplikacja dla DJ-a grającego na weselach i festynach wiejskich. Gość podchodzi do kiosku,
znajduje utwór i dopisuje dedykację. DJ ją widzi, poprawia i zatwierdza. Aplikacja czyta dedykację
syntetycznym głosem, a potem puszcza zamówiony utwór przez foobar2000.

Wszystko działa **offline, w sieci lokalnej** — bez chmury, bez telemetrii, bez zależności
od internetu. Pliki muzyczne nigdy nie są kopiowane: baza trzyma wyłącznie metadane i ścieżki.

## Jak to działa

1. **Kiosk** (dla gości) — wyszukiwanie w bibliotece, wybór utworu, dedykacja, wysłanie prośby.
   Krótki przepływ, duże elementy, zero informacji technicznych.
2. **Aplikacja DJ-a** — kolejka do przeglądu, kolejka gotowych do wykonania (z własną kolejnością)
   i historia. **Nic nie leci na antenę bez zatwierdzenia DJ-a** — żadnego autoodtwarzania.
3. **Lektor** — po zatwierdzeniu dedykacji (albo po jej poprawce) voice-over liczy się w tle,
   z widocznym statusem i paskiem postępu, i zapisuje się w pamięci podręcznej. Dzięki temu
   „Wykonaj” nie czeka na syntezę.
4. **Wykonanie** — dedykacja leci z głośników, a po niej utwór puszczony w foobar2000.

Komunikacja DJ ↔ kiosk idzie po WebSocket w sieci lokalnej i wymaga PIN-u parowania. Zarówno
DJ, jak i kiosk startują niezależnie — brak kiosku nigdy nie blokuje aplikacji DJ-a.

## Wymagania

- **Windows 11**, x86-64. Aplikacja działa też na procesorach bez AVX2.
- **foobar2000** z wtyczką **beefweb** (zdalne sterowanie przez HTTP): `http://localhost:8880`
- Wyjście audio w foobar2000 musi być w trybie **shared** — wyłączny WASAPI albo ASIO psuje
  voice-over, bo lektor i muzyka nie mogą się zmiksować.
- Kiosk i komputer DJ-a w **jednej sieci lokalnej**.

## Struktura repozytorium

```text
apps/dj          aplikacja DJ-a (Tauri v2 + React)
apps/kiosk       kiosk dla gości (Tauri v2 + React)
crates/protocol  wspólne typy wiadomości WebSocket dla obu aplikacji
tools/voices     jednorazowa konwersja głosów Piper do układu sherpa-onnx
```

Stos: Rust (edition 2024) + Tauri v2, React + TypeScript (strict) + Tailwind, SQLite, tokio.
Głosy lektora: modele **Piper** uruchamiane przez **sherpa-onnx**.

## Praca nad projektem

Wymagania: Node.js + npm, Rust (stable, zgodny z `rust-version` w workspace) i MSVC
oraz WebView2 na Windows.

```bash
# zależności frontendu (osobno dla każdej aplikacji)
cd apps/dj && npm install
cd ../kiosk && npm install

# testy całego workspace'u
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# uruchomienie w trybie deweloperskim
cd apps/dj && npm run tauri dev
cd apps/kiosk && npm run tauri dev

# zbudowanie uruchamialnego pliku (samo `cargo build` nie wystarczy)
cd apps/dj && npm run tauri build -- --debug --no-bundle
```

## Głosy lektora

Modele mają po ~60 MB i **nie leżą w repozytorium**. Konwersję ze surowych głosów Piper do układu
`sherpa-onnx` robi się raz, **na maszynie budującej aplikację** — instrukcja i skrypt są
w [`tools/voices/README.md`](tools/voices/README.md). Gotowe głosy wchodzą do instalatora jako
zasoby aplikacji, więc osoba instalująca ANON DJ niczego nie konwertuje i niczego nie pobiera.

Do pracy nad kodem bez instalatora wystarczy położyć skonwertowane głosy w
`%APPDATA%\pl.anondj.dj\voices`.

## Gdzie trafiają dane

- baza danych: `%APPDATA%\pl.anondj.dj\anon-dj.sqlite`
- logi: `%LOCALAPPDATA%\pl.anondj.dj\logs\`
- pamięć podręczna voice-overów: `%APPDATA%\pl.anondj.dj\tts-cache\`

## Licencja

Projekt nie ma jeszcze wybranej licencji. Osobno licencjonowane są komponenty zewnętrzne:
głosy Piper (m.in. `justyna`, `jarvis`, `meski`, `zenski` — MIT) oraz fonemizer `espeak-ng`;
ich teksty licencyjne trzeba dołączyć do instalatora.
