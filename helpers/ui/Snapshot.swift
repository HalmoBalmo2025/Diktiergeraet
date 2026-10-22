// `diktator-ui --snapshot <ordner>`: Panel und Hinweis als PNG, zum Prüfen der Gestaltung.

import AppKit
import SwiftUI

/// `diktator-ui --snapshot <ordner>`: zeichnet jeden Reiter mit
/// Beispieldaten als PNG (zum Prüfen der Gestaltung ohne Klicken).
@MainActor
func snapshot(into directory: String) {
    let model = Model()
    model.setup = Setup(key: "fn", model: "cohere-transcribe-03-2026-Q8_0.gguf", version: "0.1.0",
                        autostart: true, log: "/Users/beispiel/Library/Logs/Diktator.log")
    model.status = StatusInfo(state: .ready)
    model.note = Note(de: "12 Wörter eingefügt.", en: "12 words inserted.")
    model.history = [
        HistoryEntry(serial: 2, label: "Der Käufer ist nach § 433 II BGB verpflichtet, den Kaufpreis zu zahlen.", text: "Der Käufer ist nach § 433 II BGB verpflichtet, den Kaufpreis zu zahlen."),
        HistoryEntry(serial: 1, label: "Ich gehe heute nach Hause.", text: "Ich gehe heute nach Hause."),
    ]
    model.transcript = TranscriptInfo(state: .transcribing, title: "BGB AT Crashkurs", percent: 34, queued: 2)
    model.youtubeFrom = "12:30"
    model.youtubeTo = "20:00"
    model.settings = SettingsView(
        entries: SettingsEntries(modelFile: "cohere-transcribe-03-2026-Q8_0.gguf", secondaryLanguage: "nl",
                                 polishBackend: .local),
        models: ["cohere-transcribe-03-2026-Q5_K_M.gguf", "cohere-transcribe-03-2026-Q8_0.gguf"],
        languages: [LanguageOption(code: "en", name: "Englisch", english: "English"),
                    LanguageOption(code: "de", name: "Deutsch", english: "German"),
                    LanguageOption(code: "nl", name: "Niederländisch", english: "Dutch")],
        polishModels: [PolishModelOption(file: "gemma-4-12B-it-qat-UD-Q4_K_XL.gguf", name: "Gemma 4 12B",
                                         size: Note(de: "6,7 GB", en: "6.7 GB"))],
        restartNeeded: true)
    model.dictionary = DictionaryEntries(
        replacements: [ReplacementEntry(from: "BGBAT", to: "BGB AT"),
                       ReplacementEntry(from: "Extunknichtigkeit", to: "Ex-tunc-Nichtigkeit")],
        statutes: ["KAGB"], terms: ["Subsumo", "Gutachtenstil"])
    model.droppedFiles = [URL(fileURLWithPath: "/Users/a/Vorlesung.m4a")]
    // Deutsch hell und dunkel, Englisch nur hell (zum Prüfen der Längen).
    for (language, scheme) in [(UiLanguage.german, ColorScheme.light), (.german, .dark), (.english, .light)] {
        model.settings.entries.uiLanguage = language
        for tab in Tab.allCases {
            model.tab = tab
            let view = Panel(model: model)
                .frame(width: 360)
                .background(scheme == .dark ? Color(white: 0.17) : Color(white: 0.96))
                .environment(\.colorScheme, scheme)
            let renderer = ImageRenderer(content: view)
            renderer.scale = 2
            guard let image = renderer.nsImage, let tiff = image.tiffRepresentation,
                  let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
            else { continue }
            let suffix = language == .english ? "-en" : ""
            let name = "\(directory)/\(tab.rawValue)-\(scheme == .dark ? "dunkel" : "hell")\(suffix).png"
            FileManager.default.createFile(atPath: name, contents: png)
        }
    }
    // Der Hinweis an der Schreibmarke: Aufnahme mit Pegel, dann Verarbeitung.
    for (state, level) in [(AppState.recording, 7), (.processing, 0)] {
        model.status = StatusInfo(state: state, language: "de", mode: .raw)
        model.level = level
        let renderer = ImageRenderer(content: IndicatorView(model: model).padding(8))
        renderer.scale = 3
        guard let image = renderer.nsImage, let tiff = image.tiffRepresentation,
              let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
        else { continue }
        FileManager.default.createFile(atPath: "\(directory)/hinweis-\(state.rawValue).png", contents: png)
    }
}
