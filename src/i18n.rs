// UI language. The source strings are Polish; `tr` returns the English text when English is selected (unknown strings pass through).
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU8, Ordering},
        OnceLock,
    },
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang { Pl, En }

impl Lang {
    pub fn code(self) -> &'static str { match self { Lang::Pl => "pl", Lang::En => "en" } }
    pub fn from_code(c: &str) -> Lang { if c.eq_ignore_ascii_case("en") { Lang::En } else { Lang::Pl } }
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(l: Lang) { LANG.store(l as u8, Ordering::Relaxed); }
pub fn lang() -> Lang { if LANG.load(Ordering::Relaxed) == 1 { Lang::En } else { Lang::Pl } }

const EN: &[(&str, &str)] = &[
    // main view
    ("Połącz wszystkie kanały na jednym urządzeniu", "Link all channels to one device"),
    ("Pokaż Sonar Lite", "Show Sonar Lite"),
    ("Wyjdź", "Exit"),
    ("Mikrofon", "Microphone"),
    ("Tryb streamera", "Streamer mode"),
    ("Kanał {} jest niedostępny: w systemie nie ma urządzenia „SteelSeries Sonar - …”. Aplikacji nie da się do niego przypisać.",
     "Channel {} is unavailable: Windows has no \"SteelSeries Sonar - …\" device for it. Apps cannot be assigned to it."),
    ("Własny", "Custom"),
    ("przeciągnij tu aplikację", "drag an app here"),
    ("wszystko przypisane", "everything is assigned"),
    ("→ Domyślne (Windows)", "→ Default (Windows)"),
    ("<nieznane>", "<unknown>"),
    ("Aplikacje", "Apps"),
    ("różne", "various"),
    ("Odsłuch", "Monitor"),
    ("Wyjście dla wszystkich kanałów", "Output for all channels"),
    ("Wyjście miksu streamu", "Stream mix output"),
    ("Aplikacje do przypisania", "Apps to be routed"),
    ("niedostępne", "unavailable"),
    ("<wybierz mikrofon>", "<choose a microphone>"),
    ("Przetwarzanie", "Processing"),
    ("AKTYWNY", "ACTIVE"),
    ("WŁĄCZ", "ENABLE"),
    ("REDUKCJA SZUMÓW", "NOISE REDUCTION"),
    ("BRAMKA", "GATE"),
    ("żadna aplikacja nie używa mikrofonu", "no app is using the microphone"),
    ("Włączony", "Enabled"),
    ("Edytuj EQ…", "Edit EQ…"),
    ("Ogólne", "General"),
    ("Urządzenia i aplikacje", "Devices and apps"),
    ("O programie", "About"),
    ("Wyjdź z programu", "Quit Sonar Lite"),
    ("Wzm.", "Gain"),
    ("Częst.", "Freq"),
    ("Resetuj EQ (Flat)", "Reset EQ (Flat)"),
    ("Wszystkie presety", "All presets"),
    ("Wbudowane", "Built-in"),
    ("Moje", "Mine"),
    ("MOJE PRESETY", "MY PRESETS"),
    ("Zapisz", "Save"),
    ("Zapisz bieżący EQ jako preset", "Save the current EQ as a preset"),
    ("nazwa presetu", "preset name"),
    ("maksymalnie 10 własnych presetów - usuń któryś", "at most 10 own presets - delete one first"),
    ("„Sonar - Microphone” cichy", "\"Sonar - Microphone\" is silent"),
    ("Częst. (Hz)", "Freq. (Hz)"),
    ("Wzm. (dB)", "Gain (dB)"),
    ("typ", "type"),
    ("Typ", "Type"),
    ("Własne presety…", "My presets…"),
    ("Zapisz bieżący", "Save current"),
    ("Tabela pasm", "Band table"),
    ("Wejście", "Input"),
    ("Wyjście (domyślnie „Sonar - Microphone”)", "Output (default \"Sonar - Microphone\")"),
    ("REDUKCJA SZUMÓW AI (RNNoise)", "AI NOISE REDUCTION (RNNoise)"),
    ("rozpoznana mowa", "speech detected"),
    ("KOMPRESOR", "COMPRESSOR"),
    ("Poziom", "Level"),
    ("BRAMKA SZUMÓW", "NOISE GATE"),
    ("Próg", "Threshold"),
    ("Oblicz próg automatycznie", "Calculate the threshold automatically"),
    ("— nieznane —", "— unknown —"),
    ("Teraz gra na:", "Playing on:"),
    ("Połącz kanały: zmiana dotyczy GAME, CHAT, MEDIA i AUX naraz", "Link channels: changes apply to GAME, CHAT, MEDIA and AUX at once"),
    ("Urządzenia — kolejność = priorytet", "Devices — order = priority"),
    ("Kanał gra na PIERWSZYM dostępnym urządzeniu z listy. „Użyj” przełącza od razu; kolejność pozostałych to zapasowe urządzenia (gdy pierwsze jest wyłączone, kanał przejdzie na następne).",
     "The channel plays on the FIRST available device in the list. \"Use\" switches immediately; the order of the rest is the fallback order (when the first one is off, the channel moves to the next)."),
    ("używane", "in use"),
    ("dostępne", "available"),
    ("Usuń", "Remove"),
    ("Niżej", "Down"),
    ("Wyżej", "Up"),
    ("Użyj", "Use"),
    ("używane (nie na liście)", "in use (not on the list)"),
    ("nie na liście", "not on the list"),
    ("Dodaj na koniec", "Add to end"),
    ("TRYB STREAMERA: WŁĄCZONY", "STREAMER MODE: ON"),
    ("TRYB STREAMERA", "STREAMER MODE"),
    ("Uruchamiaj z Windows", "Start with Windows"),
    ("Odśwież", "Refresh"),
    ("Kanały Sonara nie istnieją w systemie", "Sonar channels do not exist in the system"),
    ("Windows nie widzi urządzeń „SteelSeries Sonar - Gaming/Chat/Media/Aux/Microphone”. GG wyłącza je przy zamknięciu, a Sonar Lite włącza je z powrotem sam. Jeśli ten komunikat nie znika, uruchom Sonar Lite jako administrator (wymaga tego włączanie urządzeń) i kliknij „Sprawdź ponownie”.",
     "Windows does not see the \"SteelSeries Sonar - Gaming/Chat/Media/Aux/Microphone\" devices. GG disables them when it exits and Sonar Lite switches them back on by itself. If this message does not go away, run Sonar Lite as administrator (switching devices on requires it) and click \"Check again\"."),
    ("Sprawdź ponownie", "Check again"),
    ("nazwa", "name"),
    ("siła", "strength"),
    ("Własne", "Custom"),
    // settings window
    ("Ustawienia", "Settings"),
    ("Język", "Language"),
    ("Uruchamianie", "Startup"),
    ("Start w zasobniku", "Start in the tray"),
    ("Zamknięcie chowa do zasobnika", "Closing hides to the tray"),
    ("Kanały Sonara", "Sonar channels"),
    ("Kanały działają.", "Channels are available."),
    ("Włącz kanały Sonara teraz", "Switch Sonar channels on now"),
    ("Odśwież urządzenia i aplikacje", "Refresh devices and apps"),
    ("Skróty klawiszowe", "Keyboard shortcuts"),
    ("wkrótce", "coming soon"),
    ("Dane programu", "Program data"),
    ("Otwórz folder ustawień", "Open settings folder"),
    ("Wersja", "Version"),
    ("Zmiana języka działa od razu.", "The language changes immediately."),
    ("Mikser", "Mixer"),
    ("Połącz kanały", "Link channels"),
    ("Przeciągnij: kolejność = priorytet", "Drag: order = priority"),
    ("Wyłączone", "Disabled"),
    ("urządzenie wyjściowe", "output device"),
    ("{} gra bezpośrednio na urządzeniu „{}” (ma własny wybór wyjścia). Ustaw w niej wyjście „Domyślne”, żeby przypisać ją do kanału.",
     "{} plays directly on the device \"{}\" (it has its own output choice). Set its output to \"Default\" to assign it to a channel."),
    // errors from the library modules
    ("nie znaleziono wejścia", "input device not found"),
    ("brak wyjścia „{}”", "output \"{}\" not found"),
    ("wyjście mikrofonu musi być stereo", "the mic output must be stereo"),
    ("wymagany format f32", "f32 format required"),
    ("brak urządzenia „{}”", "device \"{}\" not found"),
    ("wyjście streamu musi być stereo f32", "the stream output must be stereo f32"),
    ("brak {}", "missing {}"),
    ("kanał Sonara nie jest f32", "the Sonar channel is not f32"),
    ("nazwa presetu: litery, cyfry, spacja, - _ .", "preset name: letters, digits, space, - _ ."),
    ("schtasks /Create nie powiodło się (potrzebne uprawnienia administratora)", "schtasks /Create failed (administrator rights are needed)"),
];

fn table() -> &'static HashMap<&'static str, &'static str> {
    static T: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    T.get_or_init(|| EN.iter().copied().collect())
}

/// Translate a (Polish) UI string to the selected language.
pub fn tr(s: &str) -> String {
    if lang() == Lang::En { table().get(s).copied().unwrap_or(s).to_string() } else { s.to_string() }
}

/// Translate a template with `{}` placeholders and fill them in order.
pub fn tr_fmt(template: &str, args: &[String]) -> String {
    let t = tr(template);
    let (mut out, mut rest, mut it) = (String::new(), t.as_str(), args.iter());
    while let Some(i) = rest.find("{}") {
        out += &rest[..i];
        out += it.next().map_or("", |a| a.as_str());
        rest = &rest[i + 2..];
    }
    out + rest
}

#[cfg(test)]
mod t {
    use super::*;

    #[test]
    fn translates_only_in_english_and_fills_placeholders() {
        set_lang(Lang::Pl);
        assert_eq!(tr("Odśwież"), "Odśwież");
        set_lang(Lang::En);
        assert_eq!(tr("Odśwież"), "Refresh");
        assert_eq!(tr("coś nieznanego"), "coś nieznanego"); // unknown strings pass through
        assert_eq!(tr_fmt("brak wyjścia „{}”", &["Cable".into()]), "output \"Cable\" not found");
        assert_eq!(tr_fmt("Kanał {} jest niedostępny: w systemie nie ma urządzenia „SteelSeries Sonar - …”. Aplikacji nie da się do niego przypisać.", &["GAME".into()]).starts_with("Channel GAME is unavailable"), true);
        set_lang(Lang::Pl);
    }

    #[test]
    fn table_has_no_duplicate_or_empty_entries() {
        let mut seen = std::collections::HashSet::new();
        for (pl, en) in EN {
            assert!(!pl.is_empty() && !en.is_empty());
            assert!(seen.insert(*pl), "duplicate key: {pl}");
        }
    }
}
