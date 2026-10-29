//! Small, dependency-free UI localisation layer.
//!
//! English source text doubles as the translation key. Missing translations
//! fall back to English, so adding a language never leaves a blank label.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    #[default]
    English,
    Italiano,
    French,
    Spanish,
    German,
    Chinese,
}

impl Language {
    pub const ALL: [Self; 6] = [
        Self::English,
        Self::Italiano,
        Self::French,
        Self::Spanish,
        Self::German,
        Self::Chinese,
    ];

    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Italiano => "Italiano",
            Self::French => "Français",
            Self::Spanish => "Español",
            Self::German => "Deutsch",
            Self::Chinese => "简体中文",
        }
    }

    pub fn from_environment() -> Self {
        for variable in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(variable) {
                let value = value.to_ascii_lowercase();
                let code = value.split(['_', '-', '.']).next().unwrap_or_default();
                match code {
                    "it" => return Self::Italiano,
                    "fr" => return Self::French,
                    "es" => return Self::Spanish,
                    "de" => return Self::German,
                    "zh" => return Self::Chinese,
                    _ => {}
                }
            }
        }
        Self::English
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_language(language: Language) {
    let index = match language {
        Language::English => 0,
        Language::Italiano => 1,
        Language::French => 2,
        Language::Spanish => 3,
        Language::German => 4,
        Language::Chinese => 5,
    };
    CURRENT.store(index, Ordering::Relaxed);
}

pub fn language() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::Italiano,
        2 => Language::French,
        3 => Language::Spanish,
        4 => Language::German,
        5 => Language::Chinese,
        _ => Language::English,
    }
}

static CATALOGS: OnceLock<HashMap<Language, HashMap<String, String>>> = OnceLock::new();

fn catalogs() -> &'static HashMap<Language, HashMap<String, String>> {
    CATALOGS.get_or_init(|| {
        let mut catalogs = HashMap::new();
        for (language, json) in [
            (Language::French, include_str!("../locales/fr.json")),
            (Language::Spanish, include_str!("../locales/es.json")),
            (Language::German, include_str!("../locales/de.json")),
            (Language::Chinese, include_str!("../locales/zh.json")),
        ] {
            if let Ok(entries) = serde_json::from_str::<HashMap<String, String>>(json) {
                catalogs.insert(language, entries);
            }
        }
        catalogs
    })
}

/// Translate an owned status/error message when it is a known static key.
pub fn tr_owned(text: String) -> String {
    let language = language();
    let translated = if language == Language::English {
        None
    } else if language == Language::Italiano {
        italian(&text)
    } else {
        catalogs()
            .get(&language)
            .and_then(|catalog| catalog.get(&text))
            .map(String::as_str)
    };
    translated.map_or(text, str::to_owned)
}

/// Translate a source string. Unknown keys stay in English.
pub fn tr(text: &'static str) -> &'static str {
    let language = language();
    if language == Language::English {
        return text;
    }
    if language == Language::Italiano {
        return italian(text).unwrap_or(text);
    }
    catalogs()
        .get(&language)
        .and_then(|catalog| catalog.get(text))
        .map_or(text, String::as_str)
}

fn italian(text: &str) -> Option<&'static str> {
    Some(match text {
        "Open…" => "Apri…",
        "Import…" => "Importa…",
        "Export G-code…" => "Esporta G-code…",
        "Save project…" => "Salva progetto…",
        "Reload" => "Ricarica",
        "Fit" => "Adatta",
        "Undo" => "Annulla",
        "Redo" => "Ripeti",
        "Light" => "Chiaro",
        "Dark" => "Scuro",
        "Grid" => "Griglia",
        "Travel" => "Spostamenti",
        "Bed" => "Piano",
        "no file" => "nessun file",
        "Language" => "Lingua",
        "Layers" => "Livelli",
        "Selected layer" => "Livello selezionato",
        "Speed" => "Velocità",
        "Power" => "Potenza",
        "Passes" => "Passate",
        "Cut" => "Taglia",
        "Show" => "Mostra",
        "Mode" => "Modalità",
        "Line" => "Linea",
        "Fill" => "Riempimento",
        "Tabs" => "Alette",
        "Manual" => "Manuale",
        "Width" => "Larghezza",
        "Place tabs" => "Posiziona alette",
        "Up" => "Su",
        "Down" => "Giù",
        "Delete" => "Elimina",
        "cut order" => "ordine di taglio",
        "Click a shape or image · Ctrl/Shift-click for multiple · right-drag to pan" => {
            "Clicca una forma o immagine · Ctrl/Maiusc per selezionare più elementi · trascina col tasto destro per spostare"
        }
        "Corner handles scale · top handle rotates · Ctrl+Z/Ctrl+Shift+Z undo/redo" => {
            "Le maniglie degli angoli scalano · quella superiore ruota · Ctrl+Z/Ctrl+Maiusc+Z annulla/ripeti"
        }
        "Drag nodes · Ctrl-click adds · dbl-click segment inserts · Break/Join · Esc finishes" => {
            "Trascina i nodi · Ctrl-clic aggiunge · doppio clic sul segmento inserisce · Spezza/Unisci · Esc termina"
        }
        "Drag image to move · corners resize · top handle rotates" => {
            "Trascina l'immagine per spostarla · gli angoli ridimensionano · la maniglia superiore ruota"
        }
        "Fill spacing" => "Spaziatura riempimento",
        "Offset" => "Offset",
        "Concentric loops" => "Anelli concentrici",
        "Angle" => "Angolo",
        "Overlap" => "Sovrapposizione",
        "Alternate" => "Alterna",
        "Reverse each line" => "Inverti ogni linea",
        "Cross" => "Incrocia",
        "Second pass at 90°" => "Seconda passata a 90°",
        "Outline" => "Contorno",
        "Cut contour" => "Taglia contorno",
        "Job" => "Lavoro",
        "Kerf" => "Kerf",
        "Max S" => "S max",
        "Optimise travel" => "Ottimizza spostamenti",
        "Dynamic power (M4)" => "Potenza dinamica (M4)",
        "Send to laser" => "Invia al laser",
        "Frame" => "Inquadra",
        "Job history" => "Cronologia lavori",
        "Clear history" => "Cancella cronologia",
        "Persistent job history file" => "File cronologia lavori",
        "Machine" => "Macchina",
        "Endpoint" => "Indirizzo",
        "Connect" => "Connetti",
        "Disconnect" => "Disconnetti",
        "Status" => "Stato",
        "not connected" => "non connesso",
        "Jog" => "Movimento",
        "Step" => "Passo",
        "Feed" => "Avanzamento",
        "Set zero (G92)" => "Zero temporaneo (G92)",
        "Save zero (G54)" => "Salva zero (G54)",
        "Control" => "Controllo",
        "Home ($H)" => "Home ($H)",
        "Unlock ($X)" => "Sblocca ($X)",
        "Stop" => "Arresta",
        "Jogging and homing move the head. Clear the bed first." => {
            "Il movimento manuale e l'home spostano la testa. Libera il piano prima."
        }
        "Selection" => "Selezione",
        "Size" => "Dimensioni",
        "Group" => "Raggruppa",
        "Ungroup" => "Separa",
        "Align" => "Allinea",
        "Distribute" => "Distribuisci",
        "Duplicate" => "Duplica",
        "Array" => "Matrice",
        "Cols" => "Colonne",
        "Rows" => "Righe",
        "Create" => "Crea",
        "Move" => "Sposta",
        "Copy" => "Copia",
        "Position" => "Posizione",
        "Ref" => "Rif.",
        "Scale" => "Scala",
        "Rotate" => "Ruota",
        "Edit nodes" => "Modifica nodi",
        "Break" => "Spezza",
        "Join" => "Unisci",
        "Cancel" => "Annulla",
        "Close" => "Chiudi",
        "Apply" => "Applica",
        "Settings" => "Impostazioni",
        "Refresh" => "Aggiorna",
        "Home" => "Home",
        "PDF page" => "Pagina PDF",
        "Open" => "Apri",
        "Send" => "Invia",
        "Use the light interface and canvas" => "Usa l'interfaccia e il piano di lavoro chiari",
        "Add SVG, DXF or image layers to the current design" => {
            "Aggiungi livelli SVG, DXF o immagine al progetto corrente"
        }
        "Undo last edit (Ctrl+Z)" => "Annulla l'ultima modifica (Ctrl+Z)",
        "Redo last undone edit (Ctrl+Shift+Z)" => "Ripeti l'ultima modifica annullata (Ctrl+Maiusc+Z)",
        "New layer" => "Nuovo livello",
        "Layer" => "Livello",
        "Jobs" => "Lavori",
        "completed" => "completato",
        "stopped" => "arrestato",
        "failed" => "fallito",
        "Settings…" => "Impostazioni…",
        "Connecting…" => "Connessione…",
        "Pause" => "Pausa",
        "Resume" => "Riprendi",
        "Controller settings" => "Impostazioni controller",
        "Home the machine?" => "Mandare la macchina in home?",
        "The head will move to the limit switches. Make sure the machine is clear." => {
            "La testa si sposterà verso i finecorsa. Assicurati che la macchina sia libera."
        }
        "Send job to the laser?" => "Inviare il lavoro al laser?",
        "No layers yet. Open an SVG or image, or drop one here." => {
            "Nessun livello. Apri un SVG o un'immagine, oppure trascinala qui."
        }
        "To" => "A",
        "Choose layer" => "Scegli livello",
        "Done" => "Fatto",
        "Delete node" => "Elimina nodo",
        "Snap" => "Aggancia",
        "Geometry" => "Geometria",
        "No design loaded" => "Nessun progetto caricato",
        "Open an SVG, or drop one onto the window." => "Apri un SVG o trascinalo nella finestra.",
        "No jobs sent yet. Completed, stopped and framing runs appear here." => {
            "Nessun lavoro inviato. I lavori completati, arrestati e le inquadrature appaiono qui."
        }
        "State" => "Stato",
        "controller settings unavailable ($$)" => "impostazioni controller non disponibili ($$)",
        "Cancel jog (realtime 0x85)" => "Annulla jog (realtime 0x85)",
        "Pause requires M4 + $32=1; use Stop." => "La pausa richiede M4 + $32=1; usa Arresta.",
        "Trace the job bounds at 3000 mm/min with the laser off." => {
            "Traccia i limiti del lavoro a 3000 mm/min con il laser spento."
        }
        "Clear a GRBL alarm." => "Cancella un allarme GRBL.",
        "Feed hold and soft reset; switches the laser off." => {
            "Pausa avanzamento e reset software; spegne il laser."
        }
        "connect to edit controller settings" => "connetti per modificare le impostazioni del controller",
        "settings unavailable; press Refresh" => "impostazioni non disponibili; premi Aggiorna",
        "Create a new empty layer" => "Crea un nuovo livello vuoto",
        "Include this layer in the job" => "Includi questo livello nel lavoro",
        "Include this layer in the job and frame" => "Includi questo livello nel lavoro e nell'inquadratura",
        "Draw this layer in the editor" => "Disegna questo livello nell'editor",
        "Select a layer to edit it." => "Seleziona un livello da modificare.",
        "Select and transform these shapes together" => "Seleziona e trasforma queste forme insieme",
        "Remove the groups these shapes belong to" => "Rimuovi i gruppi a cui appartengono queste forme",
        "Align left edges" => "Allinea i bordi sinistri",
        "Align horizontal centres" => "Allinea i centri orizzontali",
        "Align right edges" => "Allinea i bordi destri",
        "Align top edges" => "Allinea i bordi superiori",
        "Align vertical centres" => "Allinea i centri verticali",
        "Align bottom edges" => "Allinea i bordi inferiori",
        "Space centres evenly left to right" => "Distribuisci i centri da sinistra a destra",
        "Space centres evenly bottom to top" => "Distribuisci i centri dal basso verso l'alto",
        "Move the selected objects to this layer" => "Sposta gli oggetti selezionati su questo livello",
        "Copy the selected objects into this layer" => "Copia gli oggetti selezionati in questo livello",
        "Delete the selected objects (Del)" => "Elimina gli oggetti selezionati (Canc)",
        "Duplicate the selected objects in place (Ctrl+D)" => "Duplica gli oggetti selezionati sul posto (Ctrl+D)",
        "Create the array and select its copies" => "Crea la matrice e seleziona le copie",
        "Move the selected reference point to these coordinates" => {
            "Sposta il punto di riferimento selezionato su queste coordinate"
        }
        "Snap dragged nodes to the canvas grid" => "Aggancia i nodi trascinati alla griglia del piano",
        "Delete the selected anchor (Del)" => "Elimina l'ancora selezionata (Canc)",
        "Velocity-scaled power. Required for a safe pause when $32=1." => {
            "Potenza proporzionale alla velocità. Necessaria per una pausa sicura con $32=1."
        }
        "Page" => "Pagina",
        "waiting for status…" => "in attesa di stato…",
        "waiting for the link…" => "in attesa del collegamento…",
        "Sending…" => "Invio…",
        "Framing…" => "Inquadratura…",
        "{} layers · {} shapes{} · {} cuts" => "{} livelli · {} forme{} · {} tagli",
        "cut {:.1} mm · travel {:.1} mm · ~{}" => {
            "taglio {:.1} mm · spostamento {:.1} mm · ~{}"
        }
        "image" => "immagine",
        "images" => "immagini",
        "connected" => "connesso",
        "connecting" => "connessione",
        "offline" => "offline",
        "h" => "h",
        "min" => "min",
        "s" => "s",
        "layers" => "livelli",
        "shape" => "forma",
        "shapes" => "forme",
        "shown" => "visibili",
        "line" => "linea",
        "fill" => "riempimento",
        "raster" => "raster",
        "contours" => "contorni",
        "cuts" => "tagli",
        "cut" => "taglio",
        "travel" => "spostamento",
        "page" => "pagina",
        "of cutting" => "di taglio",
        "highest power" => "potenza massima",
        "select objects to copy" => "seleziona oggetti da copiare",
        "clipboard does not contain SVG objects" => "gli appunti non contengono oggetti SVG",
        "clipboard SVG contains no paths; outline text first" => "l'SVG negli appunti non contiene tracciati; converti prima il testo in tracciati",
        "clipboard SVG contains no visible geometry" => "l'SVG negli appunti non contiene geometria visibile",
        "cannot place pasted objects" => "impossibile posizionare gli oggetti incollati",
        "nothing to import" => "niente da importare",
        "disconnected" => "disconnesso",
        "framing job bounds (laser off)…" => "traccia i limiti del lavoro (laser spento)…",
        "framing complete" => "inquadratura completata",
        "framing stopped" => "inquadratura interrotta",
        "enter an endpoint, e.g. tcp://192.168.1.10" => "inserisci un indirizzo, ad es. tcp://192.168.1.10",
        "machine thread stopped" => "thread della macchina arrestato",
        "nothing to send: the job has no cuts" => "niente da inviare: il lavoro non ha tagli",
        "sending job…" => "invio del lavoro…",
        "nothing to frame: no layers are set to Cut" => "niente da inquadrare: nessun livello è impostato su Taglia",
        "refresh controller settings first" => "aggiorna prima le impostazioni del controller",
        "applying controller settings…" => "applicazione impostazioni controller…",
        "no controller settings changed" => "nessuna impostazione del controller modificata",
        "select objects to delete" => "seleziona oggetti da eliminare",
        "select a layer to delete" => "seleziona un livello da eliminare",
        "only empty layers can be deleted" => "si possono eliminare solo livelli vuoti",
        "select objects to duplicate" => "seleziona oggetti da duplicare",
        "array spacing must be a finite number" => "la spaziatura della matrice deve essere un numero finito",
        "an array needs at least two positions" => "una matrice richiede almeno due posizioni",
        "array is too large (maximum 500 positions)" => "matrice troppo grande (massimo 500 posizioni)",
        "select objects to move or copy" => "seleziona oggetti da spostare o copiare",
        "choose a target layer" => "scegli un livello di destinazione",
        "cannot put vector objects on a raster layer" => "impossibile inserire oggetti vettoriali in un livello raster",
        "nothing to move or copy" => "niente da spostare o copiare",
        "select at least two shapes to group" => "seleziona almeno due forme da raggruppare",
        "selection is not grouped" => "la selezione non è raggruppata",
        "already aligned" => "già allineato",
        "select at least three objects to distribute" => "seleziona almeno tre oggetti da distribuire",
        "already evenly spaced" => "già distribuito uniformemente",
        "nothing to undo" => "niente da annullare",
        "undid edit" => "modifica annullata",
        "nothing to redo" => "niente da ripristinare",
        "redid edit" => "modifica ripristinata",
        "target position is not a finite number" => "la posizione di destinazione non è un numero finito",
        "selection is already at that position" => "la selezione è già in quella posizione",
        "added node" => "nodo aggiunto",
        "select one anchor to delete" => "seleziona un'ancora da eliminare",
        "select an anchor to delete" => "seleziona un'ancora da eliminare",
        "cannot delete this node" => "impossibile eliminare questo nodo",
        "deleted node" => "nodo eliminato",
        "cannot break at this node" => "impossibile spezzare in questo nodo",
        "select two endpoint anchors to join" => "seleziona due ancore finali da unire",
        "cannot join these nodes" => "impossibile unire questi nodi",
        "select exactly one shape to edit its nodes" => "seleziona esattamente una forma per modificarne i nodi",
        "node editing: drag anchors and control points" => "modifica nodi: trascina ancore e punti di controllo",
        "node editing finished" => "modifica nodi terminata",
        "tab editing finished" => "modifica alette terminata",
        "click a closed contour to place a tab" => "clicca un contorno chiuso per inserire un'aletta",
        "removed tab" => "aletta rimossa",
        "added tab" => "aletta aggiunta",
        "nothing to export" => "niente da esportare",
        "nothing to reload" => "niente da ricaricare",
        _ => return None,
    })
}

const PREFERENCES_FORMAT: u32 = 2;

#[derive(Serialize, Deserialize, Default)]
struct Preferences {
    #[serde(default)]
    format: u32,
    language: Option<Language>,
}

fn preferences_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("orleans").join("ui.json")
}

pub fn load_language() -> Option<Language> {
    let text = fs::read_to_string(preferences_path()).ok()?;
    let preferences = serde_json::from_str::<Preferences>(&text).ok()?;
    (preferences.format >= PREFERENCES_FORMAT)
        .then_some(preferences.language)
        .flatten()
}

pub fn save_language(language: Language) {
    let path = preferences_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let preferences = Preferences {
        format: PREFERENCES_FORMAT,
        language: Some(language),
    };
    if let Ok(json) = serde_json::to_string_pretty(&preferences) {
        let _ = fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_cover_core_labels_and_fall_back() {
        assert_eq!(italian("Cut"), Some("Taglia"));
        assert_eq!(italian("Layers"), Some("Livelli"));
        assert_eq!(italian("not a real label"), None);

        let previous = language();
        set_language(Language::French);
        assert_eq!(tr("Layers"), "Calques");
        set_language(Language::Spanish);
        assert_eq!(tr("Layers"), "Capas");
        set_language(Language::German);
        assert_eq!(tr("Layers"), "Ebenen");
        set_language(Language::Chinese);
        assert_eq!(tr("Layers"), "图层");
        assert_eq!(tr("not a real label"), "not a real label");
        set_language(previous);
    }
}
