#!/usr/bin/env python3
"""Konwersja głosów Piper do układu, którego oczekuje aplikacja DJ-a.

Piper publikuje głosy jako parę `*.onnx` + `*.onnx.json`. `sherpa-onnx` (silnik lektora w ANON DJ)
potrzebuje trzech rzeczy, których Piper nie daje wprost:

1. metadanych **wewnątrz** pliku `.onnx` — inaczej nie wie, że to model VITS z fonemizacją espeak;
2. osobnego pliku `tokens.txt` — Piper trzyma tę tablicę w `phoneme_id_map` w `.onnx.json`;
3. wspólnego katalogu `espeak-ng-data` — jednego dla wszystkich głosów.

Układ wyjściowy jest dokładnie taki, jaki czyta `apps/dj/src-tauri/src/piper.rs`:

    <katalog głosów>/
      espeak-ng-data/          <- wspólne dla wszystkich głosów
      <id>/
        model.onnx
        tokens.txt
        voice.json             <- nazwa, jakość i źródło do listy w ustawieniach

Identyfikator to krótka nazwa głosu: `pl_PL-justyna_wg_glos-medium` staje się `justyna`.
To ten sam zapis, którego używa PLAN.md i domyślne ustawienie aplikacji.

Skrypt uruchamia się raz, przy przygotowaniu głosów. Modele nie trafiają do repozytorium
(po ~60 MB na głos) — trzymamy tylko ten skrypt.

Wymagania: `pip install onnx`.

Przykład:

    python tools/voices/convert.py \\
        --source .tmp/tts-probe/voices \\
        --dest "%APPDATA%/pl.anondj.dj/voices" \\
        --espeak-ng-data .tmp/tts-probe/sherpa/espeak-ng-data \\
        --only pl_PL-justyna_wg_glos-medium,pl_PL-jarvis_wg_glos-medium

Gdy nie masz jeszcze `espeak-ng-data`, dodaj `--download-espeak` — skrypt pobierze wspólną
paczkę z wydań sherpa-onnx.
"""

import argparse
import json
import shutil
import sys
import tarfile
import tempfile
import time
import urllib.request
from pathlib import Path

# Wspólne dane espeak-ng dla wszystkich głosów Piper (wydanie sherpa-onnx).
ESPEAK_URL = (
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/espeak-ng-data.tar.bz2"
)

# Nazwy plików w katalogu głosu — muszą zgadzać się z `piper.rs`.
MODEL_FILE = "model.onnx"
TOKENS_FILE = "tokens.txt"
METADATA_FILE = "voice.json"
DATA_DIR = "espeak-ng-data"

# Sufiksy jakości w nazwach modeli Piper.
QUALITY_SUFFIXES = ("-low", "-medium", "-high")

# Sufiksy mówiące, skąd wzięło się nagranie głosu — nie są częścią jego nazwy.
SOURCE_SUFFIXES = ("_wg_glos",)


def short_id(voice_id: str) -> str:
    """Krótki identyfikator głosu: bez kodu języka, jakości i znacznika źródła nagrań.

    `pl_PL-justyna_wg_glos-medium` staje się `justyna`. Właśnie ten identyfikator trafia do
    ustawień aplikacji (PLAN.md, sekcja 8), więc ma być krótki i stabilny.
    """
    name = voice_id

    if "-" in name:
        prefix, rest = name.split("-", 1)
        # Prefiks w rodzaju `pl_PL` to kod języka, nie część nazwy głosu.
        if "_" in prefix:
            name = rest

    for suffix in QUALITY_SUFFIXES:
        if name.lower().endswith(suffix):
            name = name[: -len(suffix)]

    for suffix in SOURCE_SUFFIXES:
        if name.lower().endswith(suffix):
            name = name[: -len(suffix)]

    return name or voice_id


def read_configs(source_dir: Path) -> list[tuple[str, Path, Path]]:
    """Zwraca pary (identyfikator, plik modelu, plik konfiguracji) dla głosów w katalogu."""
    found: list[tuple[str, Path, Path]] = []

    for onnx in sorted(source_dir.glob("*.onnx")):
        config = onnx.with_suffix(".onnx.json")

        if not config.is_file():
            print(f"  pomijam {onnx.name}: brak pliku .onnx.json")

            continue

        found.append((onnx.name[: -len(".onnx")], onnx, config))

    return found


def prettify_name(voice_id: str) -> str:
    """Nazwa do pokazania DJ-owi."""
    name = voice_id.replace("_", " ").strip()

    return name[:1].upper() + name[1:] if name else voice_id


def quality_from(voice_id: str) -> str | None:
    """Jakość zapisana w nazwie modelu, np. `-medium`."""
    lowered = voice_id.lower()

    for suffix in QUALITY_SUFFIXES:
        if lowered.endswith(suffix):
            return suffix[1:]

    return None


def write_tokens(config: dict, destination: Path) -> None:
    """Zamienia `phoneme_id_map` z konfiguracji Piper na plik `tokens.txt` sherpa-onnx."""
    id_map = config.get("phoneme_id_map")

    if not isinstance(id_map, dict) or not id_map:
        raise ValueError("konfiguracja nie zawiera phoneme_id_map")

    with destination.open("w", encoding="utf-8") as handle:
        for symbol, ids in id_map.items():
            handle.write(f"{symbol} {ids[0]}\n")


def add_metadata(onnx_path: Path, config: dict) -> None:
    """Dopisuje do modelu to, co sherpa-onnx musi o nim wiedzieć."""
    import onnx

    language = config.get("language") or {}
    espeak = config.get("espeak") or {}
    audio = config.get("audio") or {}

    metadata = {
        "model_type": "vits",
        # Ten wpis mówi sherpa-onnx, że fonemizacja idzie przez espeak-ng.
        "comment": "piper",
        "language": language.get("name_english") or language.get("code") or "unknown",
        "voice": espeak.get("voice") or "unknown",
        "has_espeak": 1,
        "n_speakers": config.get("num_speakers") or 1,
        "sample_rate": audio.get("sample_rate") or 22050,
    }

    model = onnx.load(str(onnx_path))

    for key, value in metadata.items():
        entry = model.metadata_props.add()
        entry.key = key
        entry.value = str(value)

    onnx.save(model, str(onnx_path))


def ensure_espeak_data(
    destination: Path,
    from_dir: Path | None,
    download: bool,
) -> None:
    """Wspólny katalog `espeak-ng-data`. Jedna kopia obsługuje wszystkie głosy."""
    target = destination / DATA_DIR

    if (target / "phondata").is_file() or (target / "phontab").is_file():
        print(f"  espeak-ng-data już jest: {target}")

        return

    if from_dir is not None:
        print(f"  kopiuję espeak-ng-data z {from_dir}")
        shutil.copytree(from_dir, target, dirs_exist_ok=True)

        return

    if not download:
        raise SystemExit(
            "Brak espeak-ng-data.\n"
            f"Podaj --espeak-ng-data <katalog> albo dodaj --download-espeak.\n"
            f"Paczka: {ESPEAK_URL}"
        )

    print("  pobieram espeak-ng-data z wydań sherpa-onnx")

    with tempfile.TemporaryDirectory() as tmp:
        archive = Path(tmp) / "espeak-ng-data.tar.bz2"

        with urllib.request.urlopen(ESPEAK_URL) as response, archive.open("wb") as out:
            shutil.copyfileobj(response, out)

        with tarfile.open(archive, "r:bz2") as tar:
            tar.extractall(tmp, filter="data")

        shutil.copytree(Path(tmp) / DATA_DIR, target, dirs_exist_ok=True)


def convert_one(
    short: str,
    voice_id: str,
    onnx: Path,
    config_path: Path,
    destination: Path,
    force: bool,
) -> bool:
    """Konwertuje jeden głos. `False` oznacza, że został pominięty."""
    voice_dir = destination / short
    model = voice_dir / MODEL_FILE

    if model.is_file() and not force:
        print(f"  {short}: już przerobiony, pomijam (użyj --force, żeby nadpisać)")

        return False

    config = json.loads(config_path.read_text(encoding="utf-8"))

    voice_dir.mkdir(parents=True, exist_ok=True)

    started = time.perf_counter()

    shutil.copyfile(onnx, model)
    add_metadata(model, config)
    write_tokens(config, voice_dir / TOKENS_FILE)

    metadata = {
        "name": prettify_name(short),
        # Skąd wziął się ten głos — ułatwia dojście do źródła, gdy trzeba coś sprawdzić.
        "source": voice_id,
    }

    quality = quality_from(voice_id)

    if quality is not None:
        metadata["quality"] = quality

    # Licencji nie zgadujemy — dopisz ją ręcznie, jeśli głos ją deklaruje.
    (voice_dir / METADATA_FILE).write_text(
        json.dumps(metadata, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    size = model.stat().st_size / (1024 * 1024)
    print(f"  {short}: gotowe w {time.perf_counter() - started:.1f} s ({size:.0f} MB)")

    return True


def use_utf8_output() -> None:
    """Konsola Windows bywa w cp1252 i wtedy polskie znaki wywalają nawet `--help`."""
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)

        if reconfigure is not None:
            reconfigure(encoding="utf-8", errors="replace")


def main() -> int:
    use_utf8_output()

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", required=True, type=Path, help="katalog z surowymi głosami Piper")
    parser.add_argument("--dest", required=True, type=Path, help="katalog głosów aplikacji DJ-a")
    parser.add_argument("--espeak-ng-data", type=Path, default=None, help="gotowy katalog espeak-ng-data do skopiowania")
    parser.add_argument("--download-espeak", action="store_true", help="pobierz espeak-ng-data z wydań sherpa-onnx")
    parser.add_argument("--only", default=None, help="lista identyfikatorów po przecinku (domyślnie wszystkie)")
    parser.add_argument("--force", action="store_true", help="nadpisz głosy już obecne w katalogu docelowym")

    args = parser.parse_args()

    if not args.source.is_dir():
        raise SystemExit(f"nie ma katalogu ze źródłowymi głosami: {args.source}")

    voices = read_configs(args.source)

    if args.only:
        wanted = {name.strip() for name in args.only.split(",") if name.strip()}
        voices = [voice for voice in voices if voice[0] in wanted]

        missing = wanted - {voice[0] for voice in voices}

        for name in sorted(missing):
            print(f"  uwaga: nie znaleziono głosu {name} w {args.source}")

    if not voices:
        raise SystemExit(f"w {args.source} nie ma żadnego głosu do przerobienia")

    args.dest.mkdir(parents=True, exist_ok=True)

    ensure_espeak_data(args.dest, args.espeak_ng_data, args.download_espeak)

    print(f"Głosy: {args.dest}")

    # Krótkie identyfikatory muszą być unikalne — przy zderzeniu zostajemy przy pełnej nazwie.
    used: dict[str, str] = {}
    planned: list[tuple[str, str, Path, Path]] = []

    for voice_id, onnx, config_path in voices:
        short = short_id(voice_id)

        if short in used:
            print(f"  uwaga: {voice_id} koliduje z {used[short]}, używam pełnej nazwy jako identyfikatora")
            short = voice_id

        used[short] = voice_id
        planned.append((short, voice_id, onnx, config_path))

    converted = 0

    for short, voice_id, onnx, config_path in planned:
        if convert_one(short, voice_id, onnx, config_path, args.dest, args.force):
            converted += 1

    print(f"Przerobione: {converted} z {len(voices)}")
    print("Uruchom aplikację DJ-a — lektor wypisze głosy z tego katalogu.")

    return 0


if __name__ == "__main__":
    sys.exit(main())
