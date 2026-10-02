//! Raport awarii: zapis tego, co wywaliło aplikację, żeby DJ mógł go skopiować i wysłać dalej.
//!
//! Dlaczego tak, a nie inaczej (ustalenia z DJ-em):
//!
//! - Profil release ma `panic = "abort"`, więc **żadne** odwĳanie stosu nie nastąpi, a aplikacja
//!   jest budowana bez okna konsoli (`windows_subsystem = "windows"`), więc komunikat paniki nie
//!   ma gdzie trafić. Bez tego modułu panika ginie bez śladu — dokładnie tak zniknęły dwie awarie
//!   przy próbce głosu.
//! - Hook paniki uruchamia się **przed** abortem, więc zdążymy zapisać raport. Raport piszemy
//!   własnym `File` z `sync_all`, a nie przez `tracing`: logger buforuje i przy abort bufor
//!   przepada, czyli ślad zniknąłby drugi raz, tym razem podstępniej.
//! - Nie próbujemy pokazać okna z hooka. Proces i tak zaraz zginie, a okno z wątku, który właśnie
//!   panikował, to proszenie się o zakleszczenie. Zamiast tego raport czeka na dysku, a aplikacja
//!   pokazuje go przy następnym uruchomieniu — to działa także wtedy, gdy nie było komu pokazać
//!   okna w momencie awarii.

// Dwa różne `Write`: `fmt` dla składania tekstu do `String`, `io` dla zapisu pliku.
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Nazwa pliku z raportem w katalogu logów. Jeden plik, nadpisywany — trzymamy ostatnią awarię.
const REPORT_FILE_NAME: &str = "crash-report.txt";

/// Ile ostatnich linii dziennika dołączamy jako kontekst. Bez tego raport mówi „co”, ale nie
/// „przy czym”.
const BREADCRUMB_LINES: usize = 80;

/// Prefiks plików dziennika `tracing-appender` (nazwy mają dopisek z datą).
const LOG_FILE_PREFIX: &str = "anon-dj.log";

/// Co wiemy o awarii. Wydzielone od `PanicHookInfo`, bo tego drugiego nie da się zbudować
/// w teście — a chcemy mieć pokryty dokładnie ten tekst, który dostanie DJ.
struct ReportInput {
    message: String,
    location: String,
    thread: String,
    backtrace: String,
}

/// Zapisuje uchwyt paniki, który utrwali raport przed zakończeniem procesu.
///
/// Wołane raz, na starcie, zaraz po przygotowaniu katalogu logów.
pub fn install_hook(log_dir: PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        let input = ReportInput {
            message: payload_text(info.payload()).unwrap_or_else(|| "brak komunikatu".to_string()),
            location: info
                .location()
                .map(|location| location.to_string())
                .unwrap_or_else(|| "nieznane miejsce".to_string()),
            thread: std::thread::current()
                .name()
                .unwrap_or("bez nazwy")
                .to_string(),
            backtrace: std::backtrace::Backtrace::force_capture().to_string(),
        };

        let report = render_report(&input, &log_dir);

        // Kopia na stderr przydaje się, gdy ktoś uruchomi aplikację z konsoli.
        eprintln!("{report}");

        if let Err(error) = write_report(&log_dir, &report) {
            // Nie ma gdzie tego zgłosić — hook nie może panikować, bo zjedlibyśmy oryginalną awarię.
            eprintln!("nie udało się zapisać raportu awarii: {error}");
        }
    }));
}

/// Składa treść raportu. Czysta funkcja — dzięki temu test sprawdza dokładnie to, co zobaczy DJ.
fn render_report(input: &ReportInput, log_dir: &Path) -> String {
    let mut report = String::new();

    let _ = writeln!(report, "ANON DJ — raport awarii");
    let _ = writeln!(report, "Wersja: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        report,
        "System: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(report, "Wątek: {}", input.thread);
    let _ = writeln!(report, "Miejsce: {}", input.location);
    let _ = writeln!(report, "Komunikat: {}", input.message);
    let _ = writeln!(report);
    let _ = writeln!(report, "--- ślad stosu ---");
    let _ = writeln!(report, "{}", input.backtrace);

    let crumbs = breadcrumbs(log_dir);

    if !crumbs.is_empty() {
        let _ = writeln!(report, "--- ostatnie linie dziennika ---");
        let _ = writeln!(report, "{crumbs}");
    }

    report
}

/// Wyciąga tekst z payloadu paniki. Panika z własnym komunikatem to `&str` albo `String`.
fn payload_text(payload: &(dyn std::any::Any + Send)) -> Option<String> {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return Some((*text).to_string());
    }

    payload.downcast_ref::<String>().cloned()
}

/// Ostatnie linie bieżącego dziennika — kontekst, przy którym doszło do awarii.
fn breadcrumbs(log_dir: &Path) -> String {
    let Some(latest) = newest_log_file(log_dir) else {
        return String::new();
    };

    match std::fs::read_to_string(&latest) {
        Ok(content) => tail_lines(&content, BREADCRUMB_LINES),
        Err(_) => String::new(),
    }
}

/// Najświeższy plik dziennika w katalogu.
///
/// Porównujemy nazwy, a nie czas modyfikacji, bo nazwa pliku dziennika zawiera datę
/// (`anon-dj.log.RRRR-MM-DD`), więc porządek leksykalny jest porządkiem chronologicznym.
/// Czas modyfikacji zawodzi wtedy, gdy dwa pliki powstaną w tej samej sekundzie — a dokładnie
/// tak dzieje się w teście i tak samo może zdarzyć się przy przewinięciu dziennika.
fn newest_log_file(log_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(log_dir).ok()?;

    let mut files: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();

            name.starts_with(LOG_FILE_PREFIX)
                .then(|| (name, entry.path()))
        })
        .collect();

    files.sort_by(|left, right| left.0.cmp(&right.0));

    files.pop().map(|(_, path)| path)
}

/// Ostatnie `count` linii tekstu, w kolejności z pliku.
fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);

    lines[start..].join("\n")
}

/// Ścieżka pliku z raportem. DJ dostaje ją w oknie, gdy zechce wysłać plik zamiast wklejać tekst.
pub fn report_path(log_dir: &Path) -> PathBuf {
    log_dir.join(REPORT_FILE_NAME)
}

/// Zapisuje raport tak, żeby przeżył natychmiastowe zakończenie procesu.
///
/// `sync_all` jest tu istotą sprawy: bez niego raport może zostać w buforze systemu i zniknąć
/// razem z procesem — czyli dokładnie to, przed czym ten moduł ma chronić.
pub fn write_report(log_dir: &Path, report: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(log_dir)?;

    let path = report_path(log_dir);
    let mut file = std::fs::File::create(&path)?;

    file.write_all(report.as_bytes())?;
    file.sync_all()?;

    Ok(path)
}

/// Raport z poprzedniej awarii, jeśli na nią nie odpowiedziano. `None`, gdy jest czysto.
pub fn pending_report(log_dir: &Path) -> Option<String> {
    std::fs::read_to_string(report_path(log_dir)).ok()
}

/// Zdejmuje raport po tym, jak DJ go zobaczył — kolejny start ma być czysty.
pub fn dismiss_report(log_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(report_path(log_dir)) {
        Ok(()) => Ok(()),
        // Brak pliku to nie błąd: dwa kliknięcia „Zamknij” nie mogą wywalać komendy.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Świeży katalog testowy — czyścimy go, żeby wynik nie zależał od poprzedniego przebiegu.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anon-dj-crash-{name}"));
        let _ = std::fs::remove_dir_all(&dir);

        dir
    }

    fn sample_input() -> ReportInput {
        ReportInput {
            message: "próba raportu".to_string(),
            location: "src/main.rs:42:9".to_string(),
            thread: "main".to_string(),
            backtrace: "0: pierwsza ramka".to_string(),
        }
    }

    #[test]
    fn the_report_carries_the_message_location_and_stack() {
        let dir = temp_dir("render");
        let report = render_report(&sample_input(), &dir);

        assert!(report.contains("ANON DJ — raport awarii"), "{report}");
        assert!(report.contains("Wersja:"), "{report}");
        assert!(report.contains("Komunikat: próba raportu"), "{report}");
        assert!(report.contains("Miejsce: src/main.rs:42:9"), "{report}");
        assert!(report.contains("Wątek: main"), "{report}");
        assert!(report.contains("--- ślad stosu ---"), "{report}");
        assert!(report.contains("pierwsza ramka"), "{report}");
    }

    #[test]
    fn the_report_includes_recent_log_lines_as_context() {
        let dir = temp_dir("render-logs");
        std::fs::create_dir_all(&dir).expect("katalog logów");
        // Treść jednowierszowa celowo: test sprawdza, że ostatnie linie dziennika trafiają do
        // raportu, a nie to, jak wygląda dzielenie tekstu na linie.
        std::fs::write(
            dir.join("anon-dj.log.2026-01-02"),
            "lektor gotowy, otwarcie strumienia",
        )
        .expect("log");

        let report = render_report(&sample_input(), &dir);

        assert!(
            report.contains("--- ostatnie linie dziennika ---"),
            "{report}"
        );
        assert!(report.contains("otwarcie strumienia"), "{report}");
    }

    #[test]
    fn a_report_survives_a_round_trip_through_the_disk() {
        let dir = temp_dir("round-trip");
        let report = "ANON DJ — raport awarii\nKomunikat: test";

        let path = write_report(&dir, report).expect("zapis raportu");

        assert!(path.exists());
        assert_eq!(pending_report(&dir).as_deref(), Some(report));
    }

    #[test]
    fn a_clean_start_has_no_pending_report() {
        let dir = temp_dir("clean");

        assert!(pending_report(&dir).is_none(), "brak pliku to brak awarii");
    }

    #[test]
    fn dismissing_removes_the_report_and_tolerates_a_second_call() {
        let dir = temp_dir("dismiss");
        write_report(&dir, "raport").expect("zapis raportu");

        dismiss_report(&dir).expect("pierwsze zdjęcie raportu");
        assert!(pending_report(&dir).is_none());

        // Drugie kliknięcie „Zamknij” nie może zwracać błędu.
        dismiss_report(&dir).expect("ponowne zdjęcie raportu");
    }

    #[test]
    fn breadcrumbs_come_from_the_newest_log_file() {
        let dir = temp_dir("breadcrumbs");
        std::fs::create_dir_all(&dir).expect("katalog logów");

        std::fs::write(dir.join("anon-dj.log.2026-01-01"), "stary wpis").expect("stary log");
        std::fs::write(dir.join("anon-dj.log.2026-01-02"), "nowszy wpis").expect("nowszy log");

        assert_eq!(breadcrumbs(&dir), "nowszy wpis");
    }

    #[test]
    fn breadcrumbs_are_limited_to_the_tail() {
        let text = (1..=200)
            .map(|number| number.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        let tail = tail_lines(&text, BREADCRUMB_LINES);
        let lines: Vec<&str> = tail.lines().collect();

        assert_eq!(lines.len(), BREADCRUMB_LINES);
        assert_eq!(lines[0], (200 - BREADCRUMB_LINES + 1).to_string());
        assert_eq!(lines[BREADCRUMB_LINES - 1], "200");
    }

    #[test]
    fn a_missing_log_directory_yields_no_breadcrumbs() {
        let dir = temp_dir("no-logs");

        assert!(breadcrumbs(&dir).is_empty());
    }

    #[test]
    fn the_panic_payload_text_handles_both_shapes() {
        let text: &str = "z &str";
        assert_eq!(payload_text(&text).as_deref(), Some("z &str"));

        let owned = String::from("ze Stringa");
        assert_eq!(payload_text(&owned).as_deref(), Some("ze Stringa"));

        assert!(
            payload_text(&42_u32).is_none(),
            "inny typ to brak komunikatu"
        );
    }
}
