# Głosy lektora

ANON DJ czyta dedykacje silnikiem `sherpa-onnx` na modelach **Piper**. Piper publikuje głosy jako
parę `*.onnx` + `*.onnx.json`, a `sherpa-onnx` potrzebuje innego układu — konwertuje go skrypt
`convert.py`.

Konwersję robi się **raz, na maszynie budującej aplikację** — nigdy na komputerze, przy którym gra
DJ. Gotowe głosy wchodzą potem do instalatora jako zasoby Tauri (`bundle.resources`), więc DJ
instaluje aplikację i od razu ma wszystkie głosy: bez Pythona, bez `pip install`, bez konwersji
i bez pobierania czegokolwiek. Modele mają po ~60 MB, więc **nie trafiają do repozytorium** —
w repo jest tylko ten skrypt.

Razem z instalatorem muszą polecieć **teksty licencji** głosów oraz fonemizera (`espeak-ng`) —
`voice.json` celowo nie zgaduje licencji, więc uzupełniamy to ręcznie.

## Co robi konwersja

1. Dopisuje do modelu metadane, po których `sherpa-onnx` rozpoznaje model VITS z fonemizacją espeak.
2. Zamienia tablicę `phoneme_id_map` z `.onnx.json` na osobny plik `tokens.txt`.
3. Kładzie wspólny katalog `espeak-ng-data` (jeden dla wszystkich głosów).
4. Dopisuje `voice.json` z nazwą i jakością głosu do listy w ustawieniach.

Układ wynikowy czyta `apps/dj/src-tauri/src/piper.rs`:

```text
<katalog głosów>/
  espeak-ng-data/
  justyna/
    model.onnx
    tokens.txt
    voice.json
```

Identyfikator to krótka nazwa głosu — `pl_PL-justyna_wg_glos-medium` staje się `justyna`. Ten sam
zapis trafia do ustawień aplikacji.

## Użycie

Potrzebny jest Python z pakietem `onnx`:

```bash
pip install onnx
```

Konwersja wybranych głosów:

```bash
python tools/voices/convert.py \
  --source sciezka/do/surowych/glosow \
  --dest sciezka/do/katalogu/pakowanego/przez/instalator \
  --espeak-ng-data sciezka/do/espeak-ng-data \
  --only pl_PL-justyna_wg_glos-medium,pl_PL-jarvis_wg_glos-medium
```

`--dest` wskazuje katalog, który trafia do instalatora — według planu pakujemy **wszystkie pięć
głosów**, więc bez `--only` przerabia cały zestaw znaleziony w `--source`. Powtórne uruchomienie
pomija głosy, które już są na miejscu (nadpisze je `--force`).

Ten sam skrypt przydaje się, gdy DJ chce dołożyć głos ręcznie już po instalacji — wtedy `--dest`
celuje w `%APPDATA%\pl.anondj.dj\voices`, które aplikacja też czyta (obok głosów z instalatora).

Gdy nie masz `espeak-ng-data`, dodaj `--download-espeak` — skrypt pobierze wspólną paczkę
z wydań sherpa-onnx. Razem z katalogiem głosów zajmuje ona ~7 MB.

## Skąd wziąć głosy

Głosy Piper leżą w `huggingface.co/rhasspy/piper-voices`. Polskie kandydatury to `justyna`,
`jarvis`, `meski`, `zenski` (zestaw WitoldG, MIT) i `mc_speech`.

Licencji nie zgadujemy — jeśli głos ją deklaruje, dopisz ją ręcznie do `voice.json`.
