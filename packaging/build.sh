#!/usr/bin/env bash
#
# Buduje instalator zbiorczy ANON DJ: dwa zwykłe instalatory Tauri (konsola i kiosk) plus launcher
# NSIS z okienkiem wyboru komponentów.
#
# Użycie:
#   packaging/build.sh                # buduje aplikacje i składa launcher
#   packaging/build.sh --skip-build   # składa launcher z instalatorów, które już leżą w target/
#
# Wymagania: Node/npm w aplikacjach, Rust i makensis z cache Tauri. Do zbudowania launchera nie
# trzeba niczego instalować — Tauri trzyma NSIS razem z własnym bundlerem.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SKIP_BUILD=0
if [[ "${1:-}" == "--skip-build" ]]; then
    SKIP_BUILD=1
fi

die() {
    printf '\n[blad] %s\n' "$1" >&2
    exit 1
}

# Komunikaty idą na stderr, bo `build_installer` jest wołany w `$(...)` i na stdout musi
# trafić wyłącznie ścieżka zwracanego pliku.
info() {
    printf '\n== %s\n' "$1" >&2
}

# --- ścieżki wejściowe -----------------------------------------------------------------------

LOCAL_APPDATA="$(cygpath "${LOCALAPPDATA:-$HOME/AppData/Local}")"
MAKENSIS="${MAKENSIS:-$LOCAL_APPDATA/tauri/NSIS/makensis.exe}"
FOOBAR_SETUP="${FOOBAR_SETUP:-$ROOT/.tmp/foobar-setup/foobar2000-x64_v2.26.exe}"
BEEFWEB_COMPONENT="${BEEFWEB_COMPONENT:-$ROOT/.tmp/foobar-setup/foo_beefweb-0.10.fb2k-component}"
LAUNCHER_ICON="$ROOT/apps/dj/src-tauri/icons/icon.ico"
VOICES_DIR="$ROOT/apps/dj/src-tauri/voices"

WORK="$ROOT/packaging/work"
BEEFWEB_DIR="$WORK/beefweb"
DIST="$ROOT/packaging/out"
BUNDLE_DIR="$ROOT/target/release/bundle/nsis"

[[ -x "$MAKENSIS" ]] || die "nie znalazłem makensis w $MAKENSIS (ustaw MAKENSIS=...)"
[[ -f "$FOOBAR_SETUP" ]] || die "brak instalatora foobar2000 w $FOOBAR_SETUP"
[[ -f "$BEEFWEB_COMPONENT" ]] || die "brak paczki beefweb w $BEEFWEB_COMPONENT"
[[ -f "$LAUNCHER_ICON" ]] || die "brak ikony launchera w $LAUNCHER_ICON"

# Instalator konsoli pakuje głosy lektora. Bez nich Tauri wywali bundlowanie, a my chcemy
# powiedzieć o tym wprost i podać komendę, która to naprawia.
[[ -d "$VOICES_DIR/espeak-ng-data" ]] || die "brak głosów w $VOICES_DIR — przygotuj je skryptem tools/voices/convert.py (patrz tools/voices/README.md)"

# --- rozpakowanie wtyczki beefweb ------------------------------------------------------------
# NSIS nie ma w standardowym zestawie wtyczki do ZIP-ów, a `.fb2k-component` to zwykły ZIP.
# Rozpakowujemy go tutaj i osadzamy już gotowe pliki.
# Na x64 foobar oczekuje DLL-a wyciągniętego z podkatalogu `x64` na wierzchu komponentu;
# zostawienie go w `x64/` kończy się błędem „Not a valid Win32 application”.

info "Przygotowuję wtyczkę beefweb"
rm -rf "$WORK"
mkdir -p "$BEEFWEB_DIR"

python - "$BEEFWEB_COMPONENT" "$BEEFWEB_DIR" <<'PY'
import sys
import zipfile

source, destination = sys.argv[1], sys.argv[2]

with zipfile.ZipFile(source) as archive:
    names = archive.namelist()

    for name in names:
        if name.startswith("beefweb.root/"):
            archive.extract(name, destination)

    # Biblioteka 64-bitowa leży w `x64/` — wyciągamy ją na wierzch komponentu.
    if "x64/foo_beefweb.dll" not in names:
        raise SystemExit("w paczce beefweb nie ma x64/foo_beefweb.dll")
    with archive.open("x64/foo_beefweb.dll") as handle, open(
        destination + "/foo_beefweb.dll", "wb"
    ) as target:
        target.write(handle.read())
PY

for required in "$BEEFWEB_DIR/foo_beefweb.dll" "$BEEFWEB_DIR/beefweb.root/index.html"; do
    [[ -f "$required" ]] || die "wtyczka beefweb niekompletna: brak $required"
done

# --- budowanie instalatorów per aplikacja ----------------------------------------------------

# Nazwa produktu z konfiguracji Tauri. Dzięki niej nie zgadujemy nazwy pliku instalatora
# (a oba instalatory leżą w tym samym katalogu, bo `target` jest wspólny dla workspace'u).
product_name() {
    node -e 'process.stdout.write(require(process.argv[1]).productName)' \
        "$ROOT/apps/$1/src-tauri/tauri.conf.json"
}

# Buduje instalator jednej aplikacji i wypisuje ścieżkę pliku wynikowego.
# Świeżość pliku wyznaczamy znacznikiem czasu, a nie porównaniem zawartości katalogu:
# przy ponownym buildzie nazwa instalatora jest ta sama, więc lista plików się nie zmienia.
build_installer() {
    local app="$1"
    local dir="$BUNDLE_DIR"
    local stamp="$WORK/.stamp-$app"
    local name new

    name="$(product_name "$app")"
    mkdir -p "$dir"
    : > "$stamp"

    info "Buduję instalator: $app"
    # Wynik npm kierujemy na stderr, żeby nie zanieczyścił ścieżki zwracanej przez `printf`.
    (cd "$ROOT/apps/$app" && npm run tauri build -- --bundles nsis >&2)

    new="$(find "$dir" -maxdepth 1 -name "${name}_*-setup.exe" -newer "$stamp" -print | head -1 || true)"
    rm -f "$stamp"

    if [[ -z "$new" ]]; then
        die "budowanie $app nie zostawiło nowego instalatora w $dir"
    fi

    printf '%s' "$new"
}

# Bierze najświeższy instalator danej aplikacji, który już leży w katalogu wynikowym.
newest_installer() {
    local name
    name="$(product_name "$1")"
    ls -t "$BUNDLE_DIR/${name}"_*-setup.exe 2>/dev/null | head -1 || true
}

if [[ "$SKIP_BUILD" == "0" ]]; then
    KONSOLA_SETUP="$(build_installer dj)"
    KIOSK_SETUP="$(build_installer kiosk)"
else
    # Bez budowania bierzemy pliki, które już są — po jednym na aplikację.
    KONSOLA_SETUP="$(newest_installer dj)"
    KIOSK_SETUP="$(newest_installer kiosk)"

    [[ -n "$KONSOLA_SETUP" && -n "$KIOSK_SETUP" && "$KONSOLA_SETUP" != "$KIOSK_SETUP" ]] \
        || die "--skip-build potrzebuje dwóch instalatorów w $BUNDLE_DIR"
fi

[[ -f "$KONSOLA_SETUP" ]] || die "brak instalatora konsoli: $KONSOLA_SETUP"
[[ -f "$KIOSK_SETUP" ]] || die "brak instalatora kiosku: $KIOSK_SETUP"

# --- złożenie launchera ----------------------------------------------------------------------

mkdir -p "$DIST"

# Ścieżki dla NSIS-a generujemy jako plik, a nie przez `/D`: dzięki temu nie walczymy
# z cytowaniem spacji i backslashami w wierszu poleceń.
python - "$ROOT/packaging/defines.nsh" "$KONSOLA_SETUP" "$KIOSK_SETUP" "$FOOBAR_SETUP" \
    "$BEEFWEB_DIR" "$DIST/ANON-DJ-instalator-zbiorczy.exe" "$LAUNCHER_ICON" <<'PY'
import sys

(
    destination,
    konsola,
    kiosk,
    foobar,
    beefweb,
    out_file,
    icon,
) = sys.argv[1:]


def windows(path: str) -> str:
    return path.replace("/", "\\")


with open(destination, "w", encoding="utf-8") as handle:
    handle.write("; Wygenerowane przez packaging/build.sh — nie edytować ręcznie.\n")
    handle.write(f'!define KONSOLA_SETUP "{windows(konsola)}"\n')
    handle.write(f'!define KIOSK_SETUP "{windows(kiosk)}"\n')
    handle.write(f'!define FOOBAR_SETUP "{windows(foobar)}"\n')
    handle.write(f'!define BEEFWEB_DIR "{windows(beefweb)}"\n')
    handle.write(f'!define OUT_FILE "{windows(out_file)}"\n')
    handle.write(f'!define LAUNCHER_ICON "{windows(icon)}"\n')
PY

info "Składam instalator zbiorczy"
# `MSYS2_ARG_CONV_EXCL` zostawia `/INPUTCHARSET` w spokoju — bez tego Git Bash zamienia go
# na ścieżkę i makensis zgłasza brak takiego pliku.
(cd "$ROOT/packaging" && MSYS2_ARG_CONV_EXCL="*" "$MAKENSIS" /INPUTCHARSET UTF8 launcher.nsi)

info "Gotowe: $DIST/ANON-DJ-instalator-zbiorczy.exe"
ls -la "$DIST/ANON-DJ-instalator-zbiorczy.exe"
