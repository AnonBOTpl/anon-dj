# Głosy lektora

ANON DJ czyta dedykacje silnikiem `sherpa-onnx` na modelach **Piper**. Piper publikuje głosy jako
parę `*.onnx` + `*.onnx.json`, a `sherpa-onnx` potrzebuje innego układu — konwertuje go skrypt
`convert.py`.

Konwersję robi się **raz**, przy przygotowaniu komputera DJ-a. Modele mają po ~60 MB, więc **nie
trafiają do repozytorium** — w repo jest tylko ten skrypt.

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
  --dest "%APPDATA%/pl.anondj.dj/voices" \
  --espeak-ng-data sciezka/do/espeak-ng-data \
  --only pl_PL-justyna_wg_glos-medium,pl_PL-jarvis_wg_glos-medium
```

Bez `--only` przerabia wszystkie głosy znalezione w `--source`. Powtórne uruchomienie pomija głosy,
które już są na miejscu (nadpisze je `--force`).

Gdy nie masz `espeak-ng-data`, dodaj `--download-espeak` — skrypt pobierze wspólną paczkę
z wydań sherpa-onnx. Razem z katalogiem głosów zajmuje ona ~7 MB.

## Skąd wziąć modele

Głosy Piper leżą w `huggingface.co/rhasspy/piper-voices`. Polskie kandydatury to `justyna`,
`jarvis`, `meski`, `zenski` (zestaw WitoldG, MIT) i `mc_speech`.

Licencji nie zgadujemy — jeśli głos ją deklaruje, dopisz ją ręcznie do `voice.json`.
