; ANON DJ — instalator zbiorczy.
;
; Okienko wyboru z trzema komponentami (KONSOLA, KIOSK, FOOBAR) i ciche odpalenie instalatorów
; wybranych pozycji. Sam nie instaluje niczego poza wtyczką beefweb, którą układa w profilu
; foobar2000 — foobar nie daje sposobu, żeby zrobić to z wiersza poleceń.
;
; Tauri dalej produkuje dwa zwykłe instalatory per aplikacja (WebView2, skróty, wpisy
; w „Programy i funkcje”). Ten instalator tylko je osadza i odpala. Ścieżki do nich przychodzą
; z `packaging/build.sh` przez wygenerowany plik `defines.nsh`, żeby ten skrypt nie musiał
; wiedzieć, jak nazywają się pliki wynikowe Tauri.

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "defines.nsh"

!ifndef OUT_FILE
  !error "Brak OUT_FILE w defines.nsh — buduj przez packaging/build.sh."
!endif
!ifndef LAUNCHER_ICON
  !error "Brak LAUNCHER_ICON w defines.nsh — buduj przez packaging/build.sh."
!endif

; Indeksy sekcji muszą zgadać się z kolejnością definicji poniżej. Sięgamy po nie przez
; `${IDX_*}`, tak jak wymaga tego preprocesor NSIS-a — gołe symbole nie są tu rozwijane.
; Osobne symbole (IDX_*) obok nazw sekcji (SEC_*) są potrzebne, bo nazwy sekcji NSIS
; rozpoznaje tylko wtedy, gdy nie przysłania ich `!define` o tej samej nazwie.
!define IDX_KONSOLA 0
!define IDX_KIOSK 1
!define IDX_FOOBAR 2

Name "ANON DJ"
OutFile "${OUT_FILE}"
Icon "${LAUNCHER_ICON}"
RequestExecutionLevel user
SetCompressor zlib
ShowInstDetails show

!define MUI_ABORTWARNING
!define MUI_ICON "${LAUNCHER_ICON}"
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE SprawdzKomponenty
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_LANGUAGE "Polish"

LangString OPIS_KONSOLA ${LANG_POLISH} "Konsola DJ-a: kolejki próśb, lektor czytający dedykacje i sterowanie foobar2000. Instaluje się razem z pięcioma głosami lektora (ok. 320 MB)."
LangString OPIS_KIOSK ${LANG_POLISH} "Kiosk dla gości: wyszukanie utworu i wpisanie dedykacji. Instaluj na komputerze, który stoi przy gościach."
LangString OPIS_FOOBAR ${LANG_POLISH} "foobar2000 2.26 wraz z wtyczką beefweb. Potrzebny tylko na komputerze DJ-a. Jeśli foobar jest już zainstalowany, ten komponent jest domyślnie odznaczony — nic nie nadpisujemy."

Var FoobarDir
Var BeefwebDir
Var Wynik

; Ustawia $FoobarDir na katalog instalacji foobara; puste, gdy nie znaleziono.
;
; KLUCZOWE: launcher kompiluje się jako 32-bit (x86-unicode), a foobar 64-bit zapisuje
; InstallDir pod HKLM\SOFTWARE\foobar2000 w widoku 64-bit. Bez `SetRegView 64` Windows
; przekierowuje odczyt 32-bitowca do WOW6432Node — a tam foobar nic nie zapisuje, więc
; wykrycie zawsze zawodziło. Sprawdzamy też HKCU (instalacja tylko dla użytkownika)
; oraz widok 32-bit (na wypadek 32-bitowego foobara).
Function ZnajdzFoobara
    StrCpy $FoobarDir ""

    SetRegView 64
    ReadRegStr $FoobarDir HKLM "SOFTWARE\foobar2000" "InstallDir"
    StrCmp $FoobarDir "" 0 sprawdz_plik
    ReadRegStr $FoobarDir HKCU "SOFTWARE\foobar2000" "InstallDir"
    StrCmp $FoobarDir "" 0 sprawdz_plik

    SetRegView 32
    ReadRegStr $FoobarDir HKLM "SOFTWARE\foobar2000" "InstallDir"
    StrCmp $FoobarDir "" 0 sprawdz_plik

    SetRegView 64
    StrCpy $FoobarDir ""
    Return

  sprawdz_plik:
    SetRegView 64
    ; Wpis w rejestrze może zostać po ręcznie usuniętym foobarze — wtedy nie dotykamy
    ; żadnego katalogu, bo trafilibyśmy w szczątek po starej instalacji.
    IfFileExists "$FoobarDir\foobar2000.exe" 0 wyczysc
    Return

  wyczysc:
    StrCpy $FoobarDir ""
FunctionEnd

Function .onInit
    ; foobar już zainstalowany? Nie proponujemy go ponownie: DJ może mieć własną bibliotekę,
    ; playlisty i ustawienia, których nie wolno nadpisać.
    Call ZnajdzFoobara
    ${If} $FoobarDir != ""
        SectionSetFlags ${IDX_FOOBAR} 0
    ${EndIf}
FunctionEnd

; Instalator bez ani jednego komponentu nie ma sensu — zatrzymujemy go na stronie wyboru.
; Sam FOOBAR też jest poprawnym wyborem: DJ bywa, że chce tylko dołożyć wtyczkę beefweb
; do foobara, który już ma.
Function SprawdzKomponenty
    SectionGetFlags ${IDX_KONSOLA} $0
    SectionGetFlags ${IDX_KIOSK} $1
    SectionGetFlags ${IDX_FOOBAR} $2
    IntOp $0 $0 & ${SF_SELECTED}
    IntOp $1 $1 & ${SF_SELECTED}
    IntOp $2 $2 & ${SF_SELECTED}
    IntOp $0 $0 | $1
    IntOp $0 $0 | $2
    ${If} $0 == 0
        MessageBox MB_ICONEXCLAMATION|MB_OK "Zaznacz przynajmniej jeden komponent do zainstalowania."
        Abort
    ${EndIf}
FunctionEnd

; ---- wykrycie katalogu foobara -------------------------------------------------------------
; Wołane z sekcji FOOBAR. Ustawia $BeefwebDir na docelowy katalog wtyczki.
; Puste $FoobarDir albo brak wtyczki ma dać czytelny komunikat, nie ciche pominięcie.

Section "Konsola DJ-a" SEC_KONSOLA
    SetOutPath "$PLUGINSDIR"
    File "/oname=anon-dj-setup.exe" "${KONSOLA_SETUP}"

    DetailPrint "Instaluję konsolę DJ-a…"
    ExecWait '"$PLUGINSDIR\anon-dj-setup.exe" /S' $Wynik

    ${If} $Wynik != 0
        MessageBox MB_ICONSTOP|MB_OK "Instalacja konsoli DJ-a nie powiodła się (kod $Wynik)."
        SetErrors
    ${Else}
        DetailPrint "Konsola DJ-a zainstalowana."
    ${EndIf}
SectionEnd

Section "Kiosk" SEC_KIOSK
    SetOutPath "$PLUGINSDIR"
    File "/oname=anon-kiosk-setup.exe" "${KIOSK_SETUP}"

    DetailPrint "Instaluję kiosk…"
    ExecWait '"$PLUGINSDIR\anon-kiosk-setup.exe" /S' $Wynik

    ${If} $Wynik != 0
        MessageBox MB_ICONSTOP|MB_OK "Instalacja kiosku nie powiodła się (kod $Wynik)."
        SetErrors
    ${Else}
        DetailPrint "Kiosk zainstalowany."
    ${EndIf}
SectionEnd

Section "foobar2000 + wtyczka beefweb" SEC_FOOBAR
    SetOutPath "$PLUGINSDIR"
    File "/oname=foobar2000-setup.exe" "${FOOBAR_SETUP}"

    Call ZnajdzFoobara
    StrCmp $FoobarDir "" 0 foobar_jest

        ; foobara nie ma — uruchamiamy jego instalator. Celowo z interfejsem: foobar sam pyta,
        ; gdzie się zainstalować i czy w trybie portable, a my nie zgadujemy jego decyzji ani
        ; nie cichniemy instalacji, która pisze do rejestru.
        DetailPrint "Uruchamiam instalator foobar2000 — przejdź przez niego i wróć tutaj."
        ExecWait '"$PLUGINSDIR\foobar2000-setup.exe"' $Wynik

        ; Po instalacji trzeba wykryć katalog na nowo.
        Call ZnajdzFoobara
        StrCmp $FoobarDir "" 0 foobar_jest

        MessageBox MB_ICONEXCLAMATION|MB_OK "Nie znalazłem zainstalowanego foobar2000, więc pominąłem wtyczkę beefweb.$\r$\n$\r$\nZainstaluj foobara i uruchom ten instalator jeszcze raz, zaznaczając tylko ten komponent."
        Goto beefweb_koniec

    foobar_jest:
    ; foobar już był — nie uruchamiamy jego instalatora ponownie, żeby nie nadpisać
    ; biblioteki i playlist DJ-a. Dokładamy tylko wtyczkę.
    DetailPrint "foobar2000 znaleziony w $FoobarDir — dokładam samą wtyczkę beefweb."

    ; Tryb portable trzyma profil przy programie, zwykły — w danych użytkownika.
    IfFileExists "$FoobarDir\portable_mode_enabled" 0 profil_zwykly

        StrCpy $BeefwebDir "$FoobarDir\profile\user-components-x64\foo_beefweb"
        Goto katalog_ok

    profil_zwykly:
    StrCpy $BeefwebDir "$APPDATA\foobar2000-v2\user-components-x64\foo_beefweb"

    katalog_ok:
    DetailPrint "Układam wtyczkę beefweb w $BeefwebDir"
    CreateDirectory "$BeefwebDir"
    SetOutPath "$BeefwebDir"
    File /r "${BEEFWEB_DIR}\*.*"
    DetailPrint "Wtyczka beefweb ułożona. Uruchom foobar2000, żeby ją wczytał."

    beefweb_koniec:
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
    !insertmacro MUI_DESCRIPTION_TEXT ${IDX_KONSOLA} $(OPIS_KONSOLA)
    !insertmacro MUI_DESCRIPTION_TEXT ${IDX_KIOSK} $(OPIS_KIOSK)
    !insertmacro MUI_DESCRIPTION_TEXT ${IDX_FOOBAR} $(OPIS_FOOBAR)
!insertmacro MUI_FUNCTION_DESCRIPTION_END
