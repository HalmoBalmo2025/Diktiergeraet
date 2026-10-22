// Reiter „Einstellungen“ und der Hinweis, wenn ein Neustart nötig ist.

import AppKit
import SwiftUI

/// Orange Hinweis, wenn geänderte Einstellungen erst nach einem Neustart
/// gelten.
struct RestartNotice: View {
    @ObservedObject var model: Model

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "arrow.clockwise.circle.fill")
                .font(.system(size: 20))
                .foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 6) {
                Text(model.t("Neustart nötig", "Restart needed")).font(.body.weight(.semibold))
                Text(model.t("Die Änderungen gelten erst\nnach einem Neustart.",
                             "The changes only apply\nafter a restart."))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Button {
                    send("restart")
                } label: {
                    Text(model.t("Jetzt neu starten", "Restart now")).font(.callout.weight(.semibold))
                }
                .buttonStyle(.borderedProminent)
                .tint(.orange)
                .controlSize(.small)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color.orange.opacity(0.12)))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color.orange.opacity(0.5)))
    }
}

struct SettingsTab: View {
    @ObservedObject var model: Model

    /// Wert in settings.toml, deutsche und englische Beschriftung.
    static let keys: [(String, String, String)] = [
        ("fn", "fn", "fn"), ("rechte_wahltaste", "rechte ⌥", "right ⌥"),
        ("rechte_befehlstaste", "rechte ⌘", "right ⌘"), ("rechte_controltaste", "rechte ⌃", "right ⌃"),
    ]
    static let unloadChoices = [0, 5, 10, 15, 30, 60, 120]
    /// Stufen des Schiebers „Text beim Sprechen“: Sekunden bis zum ersten
    /// Teil, zuletzt 0 = erst am Ende.
    static let partChoices = [5, 10, 15, 20, 0]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if model.settings.restartNeeded { RestartNotice(model: model) }
            SectionTitle(title: model.t("Diktat", "Dictation"))
            Card {
                Row(symbol: "globe", title: model.t("Zweite Sprache", "Second language"), detail: shortcutHint) {
                    Picker("", selection: entry(\.secondaryLanguage)) {
                        ForEach(model.secondaryLanguages, id: \.code) { language in
                            Text(model.name(of: language)).tag(language.code)
                        }
                    }
                    .labelsHidden().pickerStyle(.menu).fixedSize()
                }
                RowDivider()
                Row(symbol: "keyboard", title: model.t("Taste", "Key")) {
                    Picker("", selection: entry(\.key)) {
                        ForEach(Self.keys, id: \.0) { key in Text(model.english ? key.2 : key.1).tag(key.0) }
                    }
                    .labelsHidden().pickerStyle(.menu).fixedSize()
                }
                RowDivider()
                partsRow
            }
            Hint(text: model.t(
                "Früh geschnitten erscheint der Text schon beim Sprechen, aber öfter mitten im Satz – dort leiden Punkt und Komma. Später oder erst am Ende: bessere Zeichensetzung.",
                "Cutting early shows the text while you speak, but more often mid-sentence, where full stops and commas suffer. Later or only at the end: better punctuation."))
            SectionTitle(title: model.t("Klänge", "Sounds"))
            Card {
                Row(symbol: "speaker.wave.2", title: model.t("Klänge", "Sounds")) {
                    Toggle("", isOn: entry(\.sounds)).toggleStyle(.switch).labelsHidden()
                }
                RowDivider()
                Row(symbol: "speaker.wave.1", title: model.t("Lautstärke", "Volume")) {
                    Slider(
                        value: Binding(
                            get: { model.volumeDraft ?? Double(model.settings.entries.volume) },
                            set: { model.volumeDraft = $0 }),
                        in: 0...100,
                        onEditingChanged: { editing in
                            guard !editing, let volume = model.volumeDraft else { return }
                            model.settings.entries.volume = Int(volume.rounded())
                            model.volumeDraft = nil
                            model.saveSettings()
                        })
                        .frame(width: 120)
                    Text("\(Int((model.volumeDraft ?? Double(model.settings.entries.volume)).rounded())) %")
                        .font(.caption).monospacedDigit().foregroundStyle(.secondary)
                        .frame(width: 36, alignment: .trailing)
                }
                .disabled(!model.settings.entries.sounds)
            }
            SectionTitle(title: model.t("Speech-to-Text-Modell", "Speech-to-text model"))
            Card {
                Row(symbol: "waveform", title: model.t("Spracherkennung", "Speech recognition"),
                    subtitle: model.settings.entries.modelFile.hasPrefix("cohere")
                        ? "Cohere Transcribe" : nil) {
                    if modelChoices.count > 1 {
                        Picker("", selection: entry(\.modelFile)) {
                            ForEach(modelChoices, id: \.self) { file in
                                Text(shortModel(file)).tag(file)
                            }
                        }
                        .labelsHidden().pickerStyle(.menu).fixedSize()
                    } else {
                        Text(shortModel(model.settings.entries.modelFile))
                            .font(.callout).foregroundStyle(.secondary)
                    }
                }
                RowDivider()
                Row(symbol: "moon.zzz", title: model.t("Entladen nach Pause", "Unload when idle"),
                    subtitle: model.t("Gibt 2,5 GB RAM frei.", "Frees 2.5 GB of RAM.")) {
                    Picker("", selection: entry(\.unloadMinutes)) {
                        ForEach(unloadChoices, id: \.self) { minutes in
                            Text(minutes == 0 ? model.t("nie", "never") : "\(minutes) min").tag(minutes)
                        }
                    }
                    .labelsHidden().pickerStyle(.menu).fixedSize()
                }
            }
            SectionTitle(title: "Large Language Model")
            Card {
                Row(symbol: "sparkles", title: model.t("Überarbeiten", "Polishing")) {
                    Picker("", selection: polishChoice) {
                        Text("Apple Intelligence").tag("apple")
                        ForEach(model.settings.polishModels, id: \.self) { option in
                            Text(option.name).tag(option.file)
                        }
                        if model.settings.polishModels.isEmpty
                            && model.settings.entries.polishBackend == .local {
                            Text(model.t("Lokales Modell", "Local model")).tag("")
                        }
                    }
                    .labelsHidden().pickerStyle(.menu).fixedSize()
                }
                if model.settings.entries.polishBackend == .local {
                    if let local = model.polishModel {
                        RowDivider()
                        Row(symbol: "moon.zzz", title: model.t("Entladen nach Pause", "Unload when idle"),
                            subtitle: model.t("Gibt \(local.size.de) RAM frei.",
                                              "Frees \(local.size.en) of RAM.")) {
                            Picker("", selection: entry(\.polishUnloadMinutes)) {
                                ForEach(polishUnloadChoices, id: \.self) { minutes in
                                    Text(minutes == 0 ? model.t("nie", "never") : "\(minutes) min").tag(minutes)
                                }
                            }
                            .labelsHidden().pickerStyle(.menu).fixedSize()
                        }
                    } else {
                        RowDivider()
                        Row(symbol: "exclamationmark.triangle", title: model.t("Kein Modell gefunden", "No model found"),
                            subtitle: model.t("GGUF-Datei in den Ordner\nmodels/llm legen.",
                                              "Put a GGUF file into\nthe models/llm folder.")) {
                            Button(model.t("Erneut prüfen", "Check again")) { model.saveSettings() }
                                .buttonStyle(.link).font(.caption)
                        }
                    }
                    if !model.settings.llamaInstalled {
                        RowDivider()
                        Row(symbol: "exclamationmark.triangle", title: model.t("llama.cpp fehlt", "llama.cpp missing"),
                            subtitle: model.t("Im Terminal:\nbrew install llama.cpp", "In Terminal:\nbrew install llama.cpp")) {
                            Button(model.t("Erneut prüfen", "Check again")) { model.saveSettings() }
                                .buttonStyle(.link).font(.caption)
                        }
                    }
                }
            }
            SectionTitle(title: model.t("Allgemein", "General"))
            Card {
                Row(symbol: "power.circle", title: model.t("Beim Anmelden starten", "Start at login")) {
                    Toggle("", isOn: Binding(
                        get: { model.setup.autostart },
                        set: { send("autostart", ["on": $0]) }))
                        .toggleStyle(.switch).labelsHidden()
                }
            }
        }
    }

    /// Auswahl „Überarbeiten“: "apple" oder die Datei des lokalen Modells.
    var polishChoice: Binding<String> {
        Binding(
            get: {
                model.settings.entries.polishBackend == .local
                    ? (model.polishModel?.file ?? "") : "apple"
            },
            set: { choice in
                if choice == "apple" {
                    model.settings.entries.polishBackend = .apple
                } else {
                    model.settings.entries.polishBackend = .local
                    model.settings.entries.polishModel = choice
                }
                model.saveSettings()
            })
    }

    var polishUnloadChoices: [Int] {
        let current = model.settings.entries.polishUnloadMinutes
        return Self.unloadChoices.contains(current)
            ? Self.unloadChoices : (Self.unloadChoices + [current]).sorted()
    }

    /// Schieber „Text beim Sprechen“: ab wie vielen Sekunden ein Diktat
    /// in Teilen erscheint, ganz rechts erst am Ende. Gespeichert wird beim
    /// Loslassen.
    var partsRow: some View {
        VStack(alignment: .leading, spacing: 4) {
            Row(symbol: "text.append", title: model.t("Text beim Sprechen", "Text while speaking"),
                subtitle: partsDescription(partSeconds)) {
                Text(partSeconds == 0 ? model.t("am Ende", "at the end") : "\(partSeconds) s")
                    .font(.callout.weight(.medium)).monospacedDigit().foregroundStyle(.secondary)
            }
            HStack(spacing: 8) {
                Text(model.t("früh", "early")).font(.caption).foregroundStyle(.secondary)
                Slider(
                    value: Binding(
                        get: { model.partDraft ?? Double(partIndex(model.settings.entries.parts)) },
                        set: { model.partDraft = $0 }),
                    in: 0...Double(Self.partChoices.count - 1), step: 1,
                    onEditingChanged: { editing in
                        guard !editing, let draft = model.partDraft else { return }
                        model.settings.entries.parts = Self.partChoices[Int(draft.rounded())]
                        model.partDraft = nil
                        model.saveSettings()
                    })
                Text(model.t("am Ende", "at the end")).font(.caption).foregroundStyle(.secondary)
            }
            .padding(.leading, 46)
            .padding(.trailing, 12)
            .padding(.bottom, 10)
        }
    }

    /// Die Sekunden, die der Schieber gerade zeigt (0 = erst am Ende).
    var partSeconds: Int {
        guard let draft = model.partDraft else { return model.settings.entries.parts }
        return Self.partChoices[min(max(Int(draft.rounded()), 0), Self.partChoices.count - 1)]
    }

    /// Stufe des Schiebers zu einem Wert aus settings.toml; ein Wert von
    /// Hand dazwischen landet auf der nächsthöheren Stufe.
    func partIndex(_ seconds: Int) -> Int {
        if seconds == 0 { return Self.partChoices.count - 1 }
        return Self.partChoices.firstIndex { $0 != 0 && $0 >= seconds } ?? Self.partChoices.count - 2
    }

    func partsDescription(_ seconds: Int) -> String {
        seconds == 0
            ? model.t("Alles erst nach dem Diktat", "Everything after the dictation")
            : model.t("Ab \(seconds) s an der nächsten Satzpause", "From \(seconds) s at the next sentence pause")
    }

    /// „Mit **fn ⇧**“.
    var shortcutHint: AttributedString {
        let text = model.t("Mit **\(model.keyLabel) ⇧**", "With **\(model.keyLabel) ⇧**")
        return (try? AttributedString(markdown: text)) ?? AttributedString(text)
    }

    /// Bindung an einen Wert; jede Änderung wird gleich gespeichert.
    func entry<Value>(_ path: WritableKeyPath<SettingsEntries, Value>) -> Binding<Value> {
        Binding(
            get: { model.settings.entries[keyPath: path] },
            set: { model.settings.entries[keyPath: path] = $0; model.saveSettings() })
    }

    var modelChoices: [String] {
        let current = model.settings.entries.modelFile
        let models = model.settings.models
        return models.contains(current) ? models : models + [current]
    }

    var unloadChoices: [Int] {
        let current = model.settings.entries.unloadMinutes
        return Self.unloadChoices.contains(current)
            ? Self.unloadChoices : (Self.unloadChoices + [current]).sorted()
    }

    /// „cohere-transcribe-03-2026-Q8_0.gguf“ → „Q8_0“.
    func shortModel(_ file: String) -> String {
        let name = file.hasSuffix(".gguf") ? String(file.dropLast(5)) : file
        return name.split(separator: "-").last.map(String.init) ?? name
    }
}
