// Reiter „Info“: Sprache der Oberfläche, Version, Modelle, Rechte, Protokoll.

import AppKit
import SwiftUI

struct InfoTab: View {
    @ObservedObject var model: Model

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionTitle(title: model.t("Sprache", "Language"))
            Card {
                // Der Titel steht in beiden Sprachen auf Englisch: So findet
                // ihn auch, wer kein Deutsch liest.
                Row(symbol: "character.bubble", title: "English",
                    subtitle: model.t("Oberfläche auf Englisch anzeigen", "Show the app in English")) {
                    Toggle("", isOn: Binding(
                        get: { model.english },
                        set: {
                            model.settings.entries.uiLanguage = $0 ? .english : .german
                            model.saveSettings()
                        }))
                        .toggleStyle(.switch).labelsHidden()
                }
            }
            SectionTitle(title: "Info")
            Card {
                Row(symbol: "number", title: "Version") { value(model.setup.version) }
                RowDivider()
                Row(symbol: "waveform", title: model.t("Spracherkennung", "Speech recognition"),
                    subtitle: model.speechDescription) { EmptyView() }
                RowDivider()
                Row(symbol: "sparkles", title: model.t("Überarbeiten", "Polishing"),
                    subtitle: model.polishDescription) { EmptyView() }
            }
            SectionTitle(title: model.t("Hilfe", "Help"))
            Card {
                ActionRow(
                    symbol: "lock.shield", title: model.t("Rechte prüfen", "Check permissions"),
                    subtitle: model.t("Mikrofon, Bedienungshilfen,\nEingabeüberwachung",
                                      "Microphone, Accessibility,\nInput Monitoring")
                ) {
                    if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
                        NSWorkspace.shared.open(url)
                    }
                }
                RowDivider()
                ActionRow(
                    symbol: "globe", title: model.t("fn-Taste freigeben", "Free the fn key"),
                    subtitle: model.t("Falls fn das Emoji-Menü öffnet:\n🌐-Taste auf „Keine Aktion“ stellen.",
                                      "If fn opens the emoji picker:\nset the 🌐 key to “Do Nothing”.")
                ) {
                    if let url = URL(string: "x-apple.systempreferences:com.apple.Keyboard-Settings.extension") {
                        NSWorkspace.shared.open(url)
                    }
                }
            }
            SectionTitle(title: model.t("Protokoll", "Log"))
            Card {
                Row(symbol: "list.bullet.rectangle", title: model.t("Protokoll erstellen", "Write a log"),
                    subtitle: model.t("Zur Fehlersuche", "For troubleshooting")) {
                    Toggle("", isOn: Binding(
                        get: { model.settings.entries.log },
                        set: { model.settings.entries.log = $0; model.saveSettings() }))
                        .toggleStyle(.switch).labelsHidden()
                }
                RowDivider()
                Group {
                    ActionRow(symbol: "doc.text.magnifyingglass", title: model.t("Protokoll öffnen", "Open log")) {
                        if let log = model.setup.log {
                            NSWorkspace.shared.open(URL(fileURLWithPath: log))
                        }
                    }
                    .disabled(model.setup.log == nil)
                    RowDivider()
                    ActionRow(symbol: "trash", title: model.t("Protokoll löschen", "Clear log"),
                              subtitle: model.t("Entfernt alle bisherigen Einträge.", "Removes all entries so far."),
                              trailingSymbol: "") {
                        send("clear_log")
                    }
                }
                .disabled(!model.settings.entries.log)
                .opacity(model.settings.entries.log ? 1 : 0.4)
            }
        }
    }

    func value(_ text: String) -> some View {
        Text(text).font(.callout).foregroundStyle(.secondary)
    }
}
