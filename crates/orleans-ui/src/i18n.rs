//! Small, dependency-free UI localisation layer.
//!
//! English source text doubles as the translation key. Missing translations
//! fall back to English, so adding a language never leaves a blank label.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    English,
    Italiano,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::English, Self::Italiano];

    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Italiano => "Italiano",
        }
    }

    pub fn from_environment() -> Self {
        for variable in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(variable) {
                let value = value.to_ascii_lowercase();
                if value == "it" || value.starts_with("it_") || value.starts_with("it-") {
                    return Self::Italiano;
                }
            }
        }
        Self::English
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_language(language: Language) {
    CURRENT.store(
        match language {
            Language::English => 0,
            Language::Italiano => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn language() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::Italiano,
        _ => Language::English,
    }
}

/// Translate a source string. Unknown keys stay in English.
pub fn tr(text: &'static str) -> &'static str {
    if language() == Language::Italiano {
        italian(text).unwrap_or(text)
    } else {
        text
    }
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
        _ => return None,
    })
}

#[derive(Serialize, Deserialize, Default)]
struct Preferences {
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
    serde_json::from_str::<Preferences>(&text)
        .ok()
        .and_then(|preferences| preferences.language)
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
    fn italian_dictionary_has_core_labels_and_falls_back() {
        assert_eq!(italian("Cut"), Some("Taglia"));
        assert_eq!(italian("Layers"), Some("Livelli"));
        assert_eq!(italian("not a real label"), None);
    }
}
