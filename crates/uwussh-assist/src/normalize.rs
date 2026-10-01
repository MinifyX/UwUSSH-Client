//! Telling that two requests ask for the same thing, offline.
//!
//! "liste alle benutzer auf", "zeig mir die Nutzer" and "show all users" all
//! become the same set of words: lower case, umlauts and ß folded, filler
//! words dropped, inflections cut off, and synonyms mapped onto one word per
//! meaning. Two requests match when those sets are nearly the same (a
//! token-set similarity above [`THRESHOLD`]) and their literals — numbers,
//! paths, names the word lists do not know — are the same. No embeddings, no
//! network: the cache must answer before any model is asked.
//!
//! Showing is what a request does when it says nothing else, so the words for
//! it ("zeige", "liste", "show", "list") are fillers. Every other verb counts:
//! "lösche die Logs" never matches "zeige die Logs".

use std::collections::BTreeSet;

/// How alike two requests must be to share an answer. Jaccard similarity of
/// their token sets: 0.75 lets one extra word in four through, no more.
pub const THRESHOLD: f64 = 0.75;

/// Words that carry no meaning for a command. Already folded (ae, oe, ue, ss).
const FILLERS: &[&str] = &[
    // German
    "bitte",
    "mir",
    "mich",
    "mal",
    "doch",
    "alle",
    "allen",
    "alles",
    "aller",
    "die",
    "der",
    "das",
    "den",
    "dem",
    "des",
    "ein",
    "eine",
    "einen",
    "einem",
    "einer",
    "eines",
    "und",
    "oder",
    "auf",
    "an",
    "am",
    "in",
    "im",
    "von",
    "vom",
    "zu",
    "zum",
    "zur",
    "mit",
    "fuer",
    "nach",
    "ueber",
    "aus",
    "ist",
    "sind",
    "gibt",
    "es",
    "wie",
    "was",
    "welche",
    "welcher",
    "welches",
    "wo",
    "ich",
    "du",
    "mein",
    "meine",
    "meinen",
    "meiner",
    "dieser",
    "diese",
    "dieses",
    "hier",
    "jetzt",
    "aktuell",
    "aktuelle",
    "aktuellen",
    "gerade",
    "kann",
    "koennen",
    "kannst",
    "moechte",
    "will",
    "soll",
    "sollen",
    "wuerde",
    "bei",
    "auch",
    "nur",
    "dann",
    "da",
    "einmal",
    "viel",
    "gerne",
    "lass",
    "sehen",
    "anzeigen",
    "zeig",
    "zeige",
    "zeigen",
    "liste",
    "listen",
    "auflisten",
    "aufliste",
    "gib",
    "gebe",
    "ausgeben",
    "ausgabe",
    "pruefe",
    "pruefen",
    "checke",
    "ob",
    "sich",
    "befehl",
    "kommando",
    // English
    "the",
    "a",
    "an",
    "all",
    "every",
    "please",
    "me",
    "my",
    "show",
    "list",
    "display",
    "print",
    "give",
    "get",
    "of",
    "to",
    "for",
    "on",
    "at",
    "with",
    "from",
    "and",
    "or",
    "is",
    "are",
    "what",
    "which",
    "how",
    "currently",
    "current",
    "i",
    "you",
    "can",
    "could",
    "would",
    "want",
    "need",
    "see",
    "view",
    "check",
    "do",
    "does",
    "there",
    "that",
    "this",
    "these",
    "those",
    "some",
    "any",
    "command",
    "now",
    "just",
];

/// One word per meaning, and the words that mean it. Already folded.
const SYNONYMS: &[(&str, &[&str])] = &[
    (
        "user",
        &[
            "benutzer",
            "nutzer",
            "user",
            "users",
            "konto",
            "konten",
            "account",
            "accounts",
            "benutzerkonto",
            "benutzerkonten",
            "anwender",
            "login",
            "logins",
        ],
    ),
    ("group", &["gruppe", "gruppen", "group", "groups"]),
    (
        "process",
        &[
            "prozess",
            "prozesse",
            "process",
            "processes",
            "task",
            "tasks",
            "programme",
            "pid",
        ],
    ),
    (
        "disk",
        &[
            "festplatte",
            "festplatten",
            "platte",
            "platten",
            "disk",
            "disks",
            "speicher",
            "speicherplatz",
            "plattenplatz",
            "storage",
            "laufwerk",
            "laufwerke",
            "drive",
            "drives",
            "datentraeger",
            "festplattenspeicher",
            "partition",
            "partitionen",
            "partitions",
            "space",
        ],
    ),
    (
        "memory",
        &["arbeitsspeicher", "ram", "memory", "mem", "hauptspeicher"],
    ),
    ("file", &["datei", "dateien", "file", "files"]),
    (
        "folder",
        &[
            "ordner",
            "verzeichnis",
            "verzeichnisse",
            "directory",
            "directories",
            "folder",
            "folders",
            "dir",
            "dirs",
        ],
    ),
    (
        "delete",
        &[
            "loesche",
            "loeschen",
            "loesch",
            "entferne",
            "entfernen",
            "delete",
            "remove",
            "rm",
            "del",
            "erase",
            "wegmachen",
        ],
    ),
    (
        "create",
        &[
            "erstelle",
            "erstellen",
            "anlegen",
            "lege",
            "erzeuge",
            "erzeugen",
            "create",
            "make",
            "add",
            "hinzufuegen",
            "fuege",
        ],
    ),
    (
        "restart",
        &[
            "neustarten",
            "neustart",
            "restart",
            "reboot",
            "rebooten",
            "neu",
        ],
    ),
    (
        "start",
        &[
            "starte",
            "starten",
            "start",
            "launch",
            "run",
            "ausfuehren",
            "fuehre",
        ],
    ),
    (
        "stop",
        &[
            "stoppe",
            "stoppen",
            "stop",
            "beende",
            "beenden",
            "halte",
            "anhalten",
            "kill",
            "toete",
            "abschiessen",
            "terminate",
        ],
    ),
    (
        "shutdown",
        &[
            "herunterfahren",
            "runterfahren",
            "ausschalten",
            "shutdown",
            "poweroff",
            "fahre",
        ],
    ),
    (
        "install",
        &["installiere", "installieren", "install", "einrichten"],
    ),
    (
        "update",
        &[
            "aktualisiere",
            "aktualisieren",
            "update",
            "updates",
            "upgrade",
            "upgrades",
        ],
    ),
    (
        "find",
        &[
            "finde",
            "finden",
            "suche",
            "suchen",
            "search",
            "find",
            "locate",
            "grep",
            "durchsuche",
        ],
    ),
    (
        "size",
        &[
            "groesse",
            "size",
            "sizes",
            "belegt",
            "belegung",
            "usage",
            "verbrauch",
            "verbraucht",
            "used",
        ],
    ),
    (
        "largest",
        &["groesste", "groessten", "groesster", "largest", "biggest"],
    ),
    (
        "count",
        &[
            "zaehle", "zaehlen", "anzahl", "count", "viele", "many", "number",
        ],
    ),
    ("port", &["port", "ports"]),
    (
        "network",
        &[
            "netzwerk",
            "network",
            "ip",
            "ips",
            "interface",
            "interfaces",
            "schnittstelle",
            "schnittstellen",
            "netzwerkkarte",
            "adresse",
            "adressen",
            "address",
            "addresses",
        ],
    ),
    (
        "service",
        &[
            "dienst", "dienste", "service", "services", "daemon", "daemons", "unit", "units",
        ],
    ),
    (
        "log",
        &[
            "log",
            "logs",
            "protokoll",
            "protokolle",
            "logdatei",
            "logdateien",
            "journal",
        ],
    ),
    (
        "package",
        &[
            "paket", "pakete", "package", "packages", "software", "programm",
        ],
    ),
    (
        "free",
        &["frei", "freie", "freien", "free", "verfuegbar", "available"],
    ),
    (
        "last",
        &[
            "letzte", "letzten", "letzter", "last", "recent", "neueste", "neuesten", "newest",
            "latest", "tail",
        ],
    ),
    (
        "open",
        &[
            "offen",
            "offene",
            "offenen",
            "open",
            "listening",
            "lauschen",
            "lauschende",
        ],
    ),
    (
        "permission",
        &[
            "rechte",
            "berechtigung",
            "berechtigungen",
            "permission",
            "permissions",
            "zugriffsrechte",
        ],
    ),
    ("owner", &["besitzer", "eigentuemer", "owner", "owners"]),
    (
        "system",
        &[
            "system", "server", "rechner", "computer", "maschine", "machine", "host", "geraet",
        ],
    ),
    ("time", &["zeit", "uhrzeit", "time", "datum", "date"]),
    (
        "cpu",
        &["cpu", "prozessor", "processor", "auslastung", "load"],
    ),
    (
        "connection",
        &["verbindung", "verbindungen", "connection", "connections"],
    ),
    (
        "hidden",
        &[
            "versteckt",
            "versteckte",
            "versteckten",
            "hidden",
            "dotfiles",
        ],
    ),
    (
        "running",
        &[
            "laufend",
            "laufende",
            "laufenden",
            "running",
            "aktiv",
            "aktive",
            "active",
        ],
    ),
    (
        "sort",
        &[
            "sortiert",
            "sortiere",
            "sortieren",
            "sort",
            "sorted",
            "geordnet",
        ],
    ),
    ("line", &["zeile", "zeilen", "line", "lines"]),
    ("version", &["version", "versionen", "release"]),
    ("kernel", &["kernel", "kern"]),
    ("home", &["home", "heimverzeichnis", "homeverzeichnis"]),
    ("password", &["passwort", "kennwort", "password", "passwd"]),
    ("firewall", &["firewall", "iptables", "nftables", "ufw"]),
    ("container", &["container", "docker", "containers"]),
    ("uptime", &["uptime", "laufzeit", "betriebszeit"]),
];

/// German and English endings, longest first, cut off a word nothing above
/// knows, as long as four letters remain.
const SUFFIXES: &[&str] = &[
    "ungen", "ung", "ing", "en", "er", "es", "ed", "em", "e", "n", "s", "t",
];

/// Lower case, umlauts and ß folded to ae/oe/ue/ss, accents dropped.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            'ä' => out.push_str("ae"),
            'ö' => out.push_str("oe"),
            'ü' => out.push_str("ue"),
            'ß' => out.push_str("ss"),
            'à' | 'á' | 'â' | 'ã' | 'å' => out.push('a'),
            'è' | 'é' | 'ê' | 'ë' => out.push('e'),
            'ì' | 'í' | 'î' | 'ï' => out.push('i'),
            'ò' | 'ó' | 'ô' | 'õ' => out.push('o'),
            'ù' | 'ú' | 'û' => out.push('u'),
            'ç' => out.push('c'),
            'ñ' => out.push('n'),
            _ => out.push(c),
        }
    }
    out
}

fn synonym(word: &str) -> Option<&'static str> {
    SYNONYMS
        .iter()
        .find(|(_, words)| words.contains(&word))
        .map(|(meaning, _)| *meaning)
}

/// A token that is more than a word: a number, a path, an option, a file name.
/// These must agree exactly for two requests to match.
fn is_literal(token: &str) -> bool {
    token
        .chars()
        .any(|c| c.is_ascii_digit() || "/.~-_*:@=\\".contains(c))
}

/// One word, as the cache compares it: its meaning, or its stem, or `None`
/// for a filler.
fn canonical(word: &str) -> Option<String> {
    if word.is_empty() || FILLERS.contains(&word) {
        return None;
    }
    if is_literal(word) {
        return Some(word.to_string());
    }
    if let Some(meaning) = synonym(word) {
        return Some(meaning.to_string());
    }
    let chars = word.chars().count();
    let mut stem = None;
    for suffix in SUFFIXES {
        let Some(cut) = word.strip_suffix(suffix) else {
            continue;
        };
        if chars - suffix.len() < 4 {
            continue;
        }
        if let Some(meaning) = synonym(cut) {
            return Some(meaning.to_string());
        }
        if FILLERS.contains(&cut) {
            return None;
        }
        stem.get_or_insert(cut);
    }
    Some(stem.unwrap_or(word).to_string())
}

/// The set of tokens a request comes down to.
pub fn tokens(request: &str) -> BTreeSet<String> {
    let folded = fold(request);
    let mut set = BTreeSet::new();
    for raw in folded.split(|c: char| !(c.is_alphanumeric() || "/.~-_*:@=\\".contains(c))) {
        // Sentence punctuation around a word is not part of it.
        let word = raw.trim_matches(|c: char| ".:-_=".contains(c));
        if let Some(token) = canonical(word) {
            set.insert(token);
        }
    }
    // "starte … neu" is a restart, not a start.
    if set.contains("start") && set.contains("restart") {
        set.remove("start");
    }
    set
}

/// The normalized form of a request, as it is stored: its tokens, sorted, one
/// space apart.
pub fn normalize(request: &str) -> String {
    tokens(request).into_iter().collect::<Vec<_>>().join(" ")
}

/// Whether two words are the same allowing one typo, for words long enough
/// that one letter does not make another word.
fn close(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if is_literal(a) || is_literal(b) || a.chars().count() < 5 || b.chars().count() < 5 {
        return false;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    // Levenshtein, stopped early: only "at most one" matters.
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        if current.iter().min().copied().unwrap_or(0) > 1 {
            return false;
        }
        previous = current;
    }
    previous[b.len()] <= 1
}

/// How alike two normalized requests are, from 0 to 1. Literals that differ
/// make it 0: "die letzten 10 Zeilen" is not "die letzten 20 Zeilen".
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<&str> = a.split_whitespace().collect();
    let b: Vec<&str> = b.split_whitespace().collect();
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let literals = |set: &[&str]| -> BTreeSet<String> {
        set.iter()
            .filter(|token| is_literal(token))
            .map(|token| token.to_string())
            .collect()
    };
    if literals(&a) != literals(&b) {
        return 0.0;
    }
    let shared = a
        .iter()
        .filter(|token| b.iter().any(|other| close(token, other)))
        .count();
    let union = a.len() + b.len() - shared;
    shared as f64 / union as f64
}

/// The best match for a normalized request among candidates, if one reaches
/// [`THRESHOLD`]. Candidates are `(normalized, value)`; on a tie the first
/// wins, so pass them most recently used first.
pub fn best_match<'a, T>(
    normalized: &str,
    candidates: impl IntoIterator<Item = (&'a str, T)>,
) -> Option<(T, f64)> {
    let mut best: Option<(T, f64)> = None;
    for (other, value) in candidates {
        let score = similarity(normalized, other);
        if score >= THRESHOLD && best.as_ref().is_none_or(|(_, top)| score > *top) {
            best = Some((value, score));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: &str, b: &str) -> bool {
        similarity(&normalize(a), &normalize(b)) >= THRESHOLD
    }

    #[test]
    fn umlauts_and_case_fold() {
        assert_eq!(fold("Lösche GRÖßE Übersicht"), "loesche groesse uebersicht");
    }

    #[test]
    fn ways_of_asking_for_the_users_are_one_request() {
        let users = normalize("liste alle benutzer auf");
        assert_eq!(users, "user");
        for other in [
            "Zeig mir alle Benutzer",
            "zeige die Nutzer an",
            "Welche Benutzerkonten gibt es?",
            "show all users",
            "list the accounts please",
            "Liste aller Benutzern",
        ] {
            assert!(
                same("liste alle benutzer auf", other),
                "{other}: {}",
                normalize(other)
            );
        }
    }

    #[test]
    fn verbs_other_than_showing_count() {
        assert!(!same("zeige die logs", "lösche die logs"));
        assert!(same("lösche alle Logdateien", "remove the logs"));
        assert!(same("starte nginx neu", "restart nginx"));
        assert!(!same("starte nginx neu", "starte nginx"));
        assert!(!same("starte nginx neu", "starte apache neu"));
    }

    #[test]
    fn synonyms_across_languages() {
        assert!(same(
            "Wie viel Speicher ist auf der Festplatte frei?",
            "show free disk space"
        ));
        assert!(same("laufende Prozesse", "show running processes"));
        assert!(same("offene Ports anzeigen", "list open ports"));
        assert!(!same(
            "freier Arbeitsspeicher",
            "freier Festplattenspeicher"
        ));
    }

    #[test]
    fn literals_must_agree() {
        assert!(!same(
            "die letzten 10 Zeilen von /var/log/syslog",
            "die letzten 20 Zeilen von /var/log/syslog"
        ));
        assert!(same(
            "die letzten 10 Zeilen von /var/log/syslog",
            "last 10 lines of /var/log/syslog"
        ));
        assert!(!same("lösche /tmp/a", "lösche /tmp/b"));
    }

    #[test]
    fn one_typo_in_a_long_word_is_forgiven() {
        assert!(same("installiere postgresql", "installiere postgrsql"));
        assert!(close("postgres", "postgre"));
        assert!(!close("nginx", "apache"));
        assert!(!close("/tmp/a", "/tmp/b"));
    }

    #[test]
    fn the_best_candidate_wins_and_poor_ones_do_not() {
        let candidates = [("user", 1), ("disk free", 2), ("process running", 3)];
        let hit = best_match(&normalize("zeig mir die Benutzer"), candidates);
        assert_eq!(hit.map(|(value, _)| value), Some(1));
        assert!(best_match(&normalize("installiere htop"), candidates).is_none());
        assert!(best_match("", candidates).is_none());
    }
}
