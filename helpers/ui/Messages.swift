// Nachrichten von Diktator (JSON-Zeilen auf stdin) und Befehle an Diktator
// (JSON-Zeilen auf stdout).

import AppKit
import SwiftUI

/// Version des Formats, das diese Oberfläche liest (`PROTOCOL_VERSION` in
/// src/app/status.rs). Beispiele jeder Nachricht liegen in fixtures/ui/.
let wireProtocolVersion = 2

struct Setup: Decodable {
    /// Version des Formats, in dem Diktator schreibt.
    var protocolVersion: Int?
    /// Taste zum Diktieren, wie in settings.toml („rechte_wahltaste“).
    var key = "fn"
    var model = ""
    var version = ""
    var autostart = false
    /// Pfad des Protokolls; fehlt, wenn macOS keinen Benutzerordner nennt.
    var log: String?
}

/// Zustand des Diktats (`Status` in src/app/status.rs). Ein Wert, den es
/// hier nicht gibt, macht die Nachricht unlesbar; das steht dann im
/// Protokoll von Diktator.
enum AppState: String, Decodable {
    case loading, ready, recording, processing, polishing, transcribing, translating, problem
    case sourceLoading = "source_loading"
}

/// Roh einfügen oder überarbeiten (`Mode` in src/hotkey.rs).
enum DictationMode: String, Decodable {
    case raw, polish
}

/// Stand des langen Transkripts (`TranscriptView` in src/app/status.rs).
enum TranscriptState: String, Decodable {
    case idle, loading, transcribing, translating
}

/// Art der Quelle (`SourceKind` in src/app/source.rs).
enum SourceKind: String, Decodable {
    case youtube, file
}

/// Womit überarbeitet wird (`PolishBackend` in src/settings.rs).
enum PolishBackend: String, Codable {
    case apple
    case local = "lokal"
}

/// Sprache des Panels (`UiLanguage` in src/settings.rs).
enum UiLanguage: String, Codable {
    case german = "de"
    case english = "en"
}

/// Meldung oben im Panel in beiden Sprachen (`Note` in src/note.rs).
struct Note: Decodable, Hashable {
    var de = ""
    var en = ""
}

struct StatusInfo: Decodable {
    var state = AppState.loading
    var language: String?
    var mode: DictationMode?
    var percent: Int?
}

struct TranscriptInfo: Decodable {
    var state = TranscriptState.idle
    var kind: SourceKind?
    var title: String?
    var percent: Int?
    /// So viele warten danach noch.
    var queued: Int?
    /// Zielsprache beim Übersetzen.
    var to: String?
}

struct HistoryEntry: Decodable, Identifiable {
    let serial: UInt64
    let label: String
    /// Der ganze Text, gezeigt, solange die Maus auf dem Eintrag liegt.
    let text: String
    var id: UInt64 { serial }
}

/// Was in der Zwischenablage liegt (`ClipboardLink` in src/app/status.rs).
enum ClipboardLink: String, Decodable {
    case youtube, missing, unreadable
}

/// Was in `settings.toml` steht (Felder wie in Rust, snake_case im JSON).
struct SettingsEntries: Codable, Equatable {
    var key = "fn"
    var sounds = true
    var volume = 25
    var modelFile = ""
    var unloadMinutes = 30
    var secondaryLanguage = "en"
    /// Ab so vielen Sekunden erscheint ein Diktat in Teilen; 0 = erst am Ende.
    var parts = 5
    var log = true
    var polishBackend = PolishBackend.apple
    var polishModel = ""
    var polishUnloadMinutes = 10
    var timestamps = false
    var chapters = true
    var uiLanguage = UiLanguage.german
}

struct LanguageOption: Decodable, Hashable {
    var code: String
    /// Deutscher Name.
    var name: String
    /// Englischer Name.
    var english: String
}

/// Ein lokales Sprachmodell in models/llm/.
struct PolishModelOption: Decodable, Hashable {
    var file: String
    var name: String
    /// Größe in beiden Sprachen („6,7 GB“ / „6.7 GB“).
    var size: Note
}

struct SettingsView: Decodable {
    var entries = SettingsEntries()
    var models: [String] = []
    var languages: [LanguageOption] = []
    var polishModels: [PolishModelOption] = []
    var llamaInstalled = true
    var restartNeeded = false
}

struct ReplacementEntry: Codable, Equatable {
    var from: String
    var to: String
}

/// Was in `dictionary.toml` steht.
struct DictionaryEntries: Codable, Equatable {
    var replacements: [ReplacementEntry] = []
    var legal = true
    var style = "römisch"
    var statutes: [String] = []
    var terms: [String] = []
}

enum Incoming: Decodable {
    case setup(Setup)
    case status(StatusInfo)
    case transcript(TranscriptInfo)
    case note(Note)
    case clearNote
    case history([HistoryEntry])
    case autostart(Bool)
    case level(Int)
    case quit
    case settings(SettingsView)
    case dictionary(DictionaryEntries)
    case clipboardLink(ClipboardLink)

    enum CodingKeys: String, CodingKey { case type, data }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        switch type {
        case "setup": self = .setup(try container.decode(Setup.self, forKey: .data))
        case "status": self = .status(try container.decode(StatusInfo.self, forKey: .data))
        case "transcript": self = .transcript(try container.decode(TranscriptInfo.self, forKey: .data))
        case "note": self = .note(try container.decode(Note.self, forKey: .data))
        case "clear_note": self = .clearNote
        case "history": self = .history(try container.decode([HistoryEntry].self, forKey: .data))
        case "autostart": self = .autostart(try container.decode(Bool.self, forKey: .data))
        case "level": self = .level(try container.decode(Int.self, forKey: .data))
        case "quit": self = .quit
        case "settings": self = .settings(try container.decode(SettingsView.self, forKey: .data))
        case "dictionary": self = .dictionary(try container.decode(DictionaryEntries.self, forKey: .data))
        case "clipboard_link": self = .clipboardLink(try container.decode(ClipboardLink.self, forKey: .data))
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type, in: container, debugDescription: "unbekannte Nachricht \(type)")
        }
    }
}


/// Schreibt einen Befehl als JSON-Zeile auf die Standardausgabe.
func send(_ command: String, _ fields: [String: Any] = [:]) {
    var object = fields
    object["command"] = command
    guard var data = try? JSONSerialization.data(withJSONObject: object) else { return }
    data.append(0x0A)
    FileHandle.standardOutput.write(data)
}

/// Schickt einen Befehl mit einem Wert, der als JSON-Objekt unter `field`
/// steht (Felder in snake_case wie in Rust).
func send<Value: Encodable>(_ command: String, _ field: String, _ value: Value) {
    let encoder = JSONEncoder()
    encoder.keyEncodingStrategy = .convertToSnakeCase
    guard let data = try? encoder.encode(value),
          let object = try? JSONSerialization.jsonObject(with: data)
    else { return }
    send(command, [field: object])
}
