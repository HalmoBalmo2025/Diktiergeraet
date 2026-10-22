// Zustand des Panels: was Diktator gemeldet hat und was im Panel eingegeben wird.

import AppKit
import SwiftUI

enum Tab: String, CaseIterable, Identifiable {
    case dictation, youtube, file, dictionary, settings, info
    var id: String { rawValue }

    var symbol: String {
        switch self {
        case .dictation: "mic"
        case .youtube: "play.rectangle"
        case .file: "folder"
        case .dictionary: "book.closed"
        case .settings: "gearshape"
        case .info: "info.circle"
        }
    }

    @MainActor func title(_ model: Model) -> String {
        switch self {
        case .dictation: model.t("Diktat", "Dictation")
        case .youtube: "YouTube"
        case .file: model.t("Datei", "File")
        case .dictionary: model.t("Wörterbuch", "Dictionary")
        case .settings: model.t("Einstellungen", "Settings")
        case .info: "Info"
        }
    }
}

@MainActor
final class Model: ObservableObject {
    @Published var setup = Setup()
    @Published var status = StatusInfo()
    @Published var transcript = TranscriptInfo()
    @Published var note = Note()
    @Published var history: [HistoryEntry] = []
    @Published var tab: Tab = .dictation
    /// Pegel der laufenden Aufnahme, 0 bis 10.
    @Published var level = 0
    @Published var settings = SettingsView()
    @Published var dictionary = DictionaryEntries()
    // Eingaben, die noch nicht gespeichert sind. Sie stehen hier statt in
    // `@State`, weil `@State` ein Makro ist, das `swiftc` ohne Xcode nicht
    // auflösen kann.
    @Published var newFrom = ""
    @Published var newTo = ""
    @Published var newStatute = ""
    @Published var newTerm = ""
    @Published var volumeDraft: Double?
    /// Stellung des Schiebers „Text beim Sprechen“, solange er bewegt wird.
    @Published var partDraft: Double?
    /// Wird gerade eine Datei über die Ablagefläche bzw. über „Datei
    /// transkribieren“ gezogen?
    @Published var dropTargeted = false
    @Published var startTargeted = false
    /// Aufs Panel oder aufs Menüleisten-Symbol gezogene Dateien, die auf
    /// die Wahl der Sprache warten.
    @Published var droppedFiles: [URL] = []
    /// Bereich „von“/„bis“ je Reiter; leer = alles. Geleert, sobald ein
    /// Transkript damit startet.
    @Published var youtubeFrom = ""
    @Published var youtubeTo = ""
    @Published var fileFrom = ""
    @Published var fileTo = ""
    /// Ob in der Zwischenablage ein YouTube-Link liegt; `nil`, bis Diktator
    /// auf `check_link` geantwortet hat.
    @Published var clipboardLink: ClipboardLink?
    /// Die YouTube-Zeile, auf der die Maus liegt.
    @Published var hoveredLinkRow: String?
    /// Stand der Zwischenablage (`changeCount`) bei der letzten Prüfung. Erst
    /// wenn sich etwas Neues darin findet, fragt das Panel wieder; den Stand
    /// zu lesen, liest nicht den Inhalt.
    private var checkedClipboard: Int?
    /// Der Verlaufseintrag, dessen ganzer Text gerade daneben steht.
    @Published var shownHistory: UInt64?
    /// Der Verlaufseintrag unter der Maus und das Warten, bis er gezeigt wird.
    private var hoveredHistory: UInt64?
    private var historyDelay: Task<Void, Never>?

    /// Deutsch: Standard für Dateien, „Sprache selbst wählen“ und
    /// Übersetzungen; fn allein diktiert Deutsch.
    static let german = "de"
    /// Sprache für „Sprache selbst wählen“ im Reiter YouTube.
    @Published var youtubeLanguage = Model.german
    /// Sprache für „Datei transkribieren“.
    @Published var fileLanguage = Model.german
    /// Zielsprache für „Transkribieren und übersetzen“.
    @Published var translateTarget = Model.german

    /// Transkribiert das kopierte Video; `language` ist ein Sprachcode oder
    /// `NSNull()` (Originalsprache). Mit `translate` danach übersetzt.
    func transcribeYoutube(language: Any, translate: String? = nil) {
        send("youtube", ["language": language, "range": ["from": youtubeFrom, "to": youtubeTo],
                         "translate": translate ?? NSNull()])
    }

    /// Die Maus kommt auf eine YouTube-Zeile (`inside`) oder verlässt sie.
    func hoverLinkRow(_ row: String, inside: Bool) {
        guard inside else {
            if hoveredLinkRow == row { hoveredLinkRow = nil }
            return
        }
        hoveredLinkRow = row
        let count = NSPasteboard.general.changeCount
        guard count != checkedClipboard else { return }
        checkedClipboard = count
        clipboardLink = nil
        send("check_link")
    }

    /// Die Maus kommt auf einen Verlaufseintrag (`inside`) oder verlässt ihn.
    /// Der ganze Text erscheint nach kurzem Verweilen, damit beim
    /// Darüberstreichen nichts aufblitzt; steht schon einer, wechselt er
    /// sofort.
    func hoverHistory(_ serial: UInt64, inside: Bool) {
        guard inside else {
            guard hoveredHistory == serial else { return }
            historyDelay?.cancel()
            hoveredHistory = nil
            shownHistory = nil
            return
        }
        historyDelay?.cancel()
        hoveredHistory = serial
        if shownHistory != nil {
            shownHistory = serial
            return
        }
        historyDelay = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled, self?.hoveredHistory == serial else { return }
            self?.shownHistory = serial
        }
    }

    /// Das Panel ist zu: Nichts liegt mehr unter der Maus.
    func panelClosed() {
        historyDelay?.cancel()
        hoveredHistory = nil
        shownHistory = nil
        hoveredLinkRow = nil
    }

    /// Englische Oberfläche? (Schalter oben im Reiter „Info“, gespeichert
    /// in settings.toml.)
    var english: Bool { settings.entries.uiLanguage == .english }

    /// Text in der Sprache der Oberfläche.
    func t(_ german: String, _ english: String) -> String { self.english ? english : german }

    /// Die Taste zum Diktieren, beschriftet in der Sprache der Oberfläche
    /// („rechte ⌥“, „right ⌥“).
    var keyLabel: String {
        let key = SettingsTab.keys.first { $0.0 == setup.key }
        return key.map { english ? $0.2 : $0.1 } ?? setup.key
    }

    /// Die Meldung oben im Panel in der Sprache der Oberfläche.
    var noteText: String { t(note.de, note.en) }

    /// Alle Sprachen des Spracherkennungsmodells (auch Zielsprachen beim
    /// Übersetzen), wie Diktator sie meldet.
    var transcriptionLanguages: [LanguageOption] { settings.languages }

    /// Name einer Sprache in der Sprache der Oberfläche.
    func name(of language: LanguageOption) -> String { english ? language.english : language.name }

    /// Sprachen zur Wahl für fn + ⇧: alle außer Deutsch (das hat fn allein).
    var secondaryLanguages: [LanguageOption] {
        settings.languages.filter { $0.code != Model.german }
    }

    /// Name einer Sprache („Englisch“) zu ihrem Code.
    func languageName(_ code: String) -> String {
        transcriptionLanguages.first { $0.code == code }.map(name(of:)) ?? code
    }

    /// Transkribiert die gezogenen Dateien nacheinander (ohne gezogene die
    /// im Finder kopierten).
    func transcribeFiles(language: String, dropped: [URL] = []) {
        let files = dropped.isEmpty ? droppedFiles : dropped
        send("file", ["language": language, "paths": files.map(\.path),
                      "range": ["from": fileFrom, "to": fileTo]])
        droppedFiles = []
        // Läuft schon etwas, wartet die Datei; ihr Bereich ist dann vergeben.
        if transcript.state != .idle { clearRange(kind: .file) }
    }

    /// Leert die Bereichsfelder des Reiters „YouTube“ oder „Datei“.
    func clearRange(kind: SourceKind?) {
        if kind == .file {
            fileFrom = ""
            fileTo = ""
        } else {
            youtubeFrom = ""
            youtubeTo = ""
        }
    }

    func saveSettings() { send("set_settings", "settings", settings.entries) }

    /// Das Spracherkennungsmodell, für den Info-Reiter.
    var speechDescription: String {
        let file = setup.model
        let name = file.hasSuffix(".gguf") ? String(file.dropLast(5)) : file
        let stage = name.split(separator: "-").last.map(String.init) ?? name
        return file.hasPrefix("cohere") ? "Cohere Transcribe (\(stage))" : file
    }

    /// Das lokale Sprachmodell, das gilt: das gewählte, sonst das erste.
    var polishModel: PolishModelOption? {
        let models = settings.polishModels
        return models.first { $0.file == settings.entries.polishModel } ?? models.first
    }

    /// Womit überarbeitet wird, für den Info-Reiter.
    var polishDescription: String {
        guard settings.entries.polishBackend == .local else {
            return t("Apple Intelligence (auf dem Gerät)", "Apple Intelligence (on device)")
        }
        guard let local = polishModel else {
            return t("Lokal – kein Modell in models/llm", "Local – no model in models/llm")
        }
        return t("\(local.name) (lokal, llama.cpp)", "\(local.name) (local, llama.cpp)")
    }

    /// Sprachcode und Name der zweiten Sprache (fn + ⇧).
    var secondaryCode: String { settings.entries.secondaryLanguage }
    var secondaryName: String { languageName(secondaryCode) }
    func saveDictionary() { send("set_dictionary", "dictionary", dictionary) }

    func apply(_ message: Incoming) {
        switch message {
        case .setup(let value):
            setup = value
            if value.protocolVersion != wireProtocolVersion {
                let written = value.protocolVersion.map { String($0) } ?? "ohne Angabe"
                send("log", ["message": "Nachrichten in Version \(written), Oberfläche liest \(wireProtocolVersion)"])
            }
        case .status(let value): status = value
        case .transcript(let value):
            // Ein Transkript startet: Sein Bereich ist vergeben.
            if transcript.state == .idle && value.state != .idle { clearRange(kind: value.kind) }
            transcript = value
        case .note(let value): note = value
        case .clearNote: note = Note()
        case .history(let value): history = value
        case .autostart(let value): setup.autostart = value
        case .level(let value): level = value
        case .quit: NSApp.terminate(nil)
        case .settings(let value): settings = value
        case .dictionary(let value): dictionary = value
        case .clipboardLink(let value):
            clipboardLink = value
            // Nicht lesbar: beim nächsten Darüberfahren noch einmal fragen.
            if value == .unreadable { checkedClipboard = nil }
        }
    }

    /// Kurzer Zustand für das Panel.
    var statusText: String {
        let language = (status.language ?? "").uppercased()
        let percent = status.percent ?? 0
        return switch status.state {
        case .loading: t("Modell lädt", "Model loading")
        case .ready: t("Bereit", "Ready")
        case .recording where status.mode == .polish:
            t("Aufnahme \(language) · überarbeiten", "Recording \(language) · polish")
        case .recording: t("Aufnahme \(language)", "Recording \(language)")
        case .processing: t("Wird verarbeitet …", "Processing …")
        case .polishing: t("Wird überarbeitet …", "Polishing …")
        case .sourceLoading: t("Lädt Tonspur …", "Loading audio …")
        case .transcribing: t("Transkribiert \(percent) %", "Transcribing \(percent) %")
        case .translating: t("Übersetzt \(percent) %", "Translating \(percent) %")
        case .problem: "Problem"
        }
    }

    var statusColor: Color {
        switch status.state {
        case .ready: .green
        case .recording, .problem: .red
        case .loading, .processing, .polishing, .sourceLoading, .transcribing, .translating: .blue
        }
    }
}
