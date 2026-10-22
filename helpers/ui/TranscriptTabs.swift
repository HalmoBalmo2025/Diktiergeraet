// Reiter „YouTube“ und „Datei“ mit Fortschritt, Zeitmarken und Ablagefläche.

import AppKit
import SwiftUI

struct TranscriptProgress: View {
    @ObservedObject var model: Model
    var transcript: TranscriptInfo { model.transcript }

    var loadingText: String {
        switch (transcript.state, transcript.title) {
        case (.transcribing, let title): model.t("„\(title ?? "")“", "“\(title ?? "")”")
        case (.translating, let title):
            model.t(
                "„\(title ?? "")“ wird ins \(model.languageName(transcript.to ?? Model.german))e übersetzt",
                "Translating “\(title ?? "")” into \(model.languageName(transcript.to ?? Model.german))")
        case (_, .some(let title)): model.t("„\(title)“ wird gelesen …", "Reading “\(title)” …")
        case (_, .none):
            transcript.kind == .file
                ? model.t("Datei: Tonspur wird geladen …", "File: loading audio …")
                : model.t("YouTube: Tonspur wird geladen …", "YouTube: loading audio …")
        }
    }

    /// Mit Prozent (Transkribieren, Übersetzen) statt wanderndem Balken.
    var measured: Bool { transcript.state == .transcribing || transcript.state == .translating }

    var body: some View {
        if transcript.state != .idle {
            SectionTitle(title: model.t("Läuft", "Running"))
            Card {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Image(systemName: "waveform").foregroundStyle(Color.accentColor)
                        Text(loadingText)
                            .font(.body.weight(.medium))
                            .lineLimit(2)
                        Spacer()
                        if let percent = transcript.percent, measured {
                            Text("\(percent) %").font(.caption.weight(.semibold)).monospacedDigit()
                        }
                    }
                    if let queued = transcript.queued, queued > 0 {
                        Text(queued == 1
                             ? model.t("Danach noch 1 weitere Datei", "1 more file after this")
                             : model.t("Danach noch \(queued) weitere Dateien", "\(queued) more files after this"))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    ProgressBar(fraction: measured
                                ? Double(transcript.percent ?? 0) / 100 : nil)
                    Button {
                        send("cancel_transcript")
                    } label: {
                        Label(model.t("Abbrechen", "Cancel"), systemImage: "xmark")
                            .font(.caption.weight(.semibold))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 4)
                            .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.08)))
                    }
                    .buttonStyle(.plain)
                }
                .padding(12)
            }
        }
    }
}

/// Fortschrittsbalken; ohne Anteil (beim Laden) ein gestrichelter Balken.
struct ProgressBar: View {
    let fraction: Double?

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(Color.primary.opacity(0.1))
                if let fraction {
                    Capsule()
                        .fill(Color.accentColor)
                        .frame(width: max(6, geometry.size.width * min(1, fraction)))
                } else {
                    // Unbestimmter Fortschritt: ein Stück Balken wandert hin und her.
                    TimelineView(.animation) { timeline in
                        let phase = timeline.date.timeIntervalSinceReferenceDate
                            .truncatingRemainder(dividingBy: 1.6) / 1.6
                        let width = geometry.size.width * 0.3
                        let travel = geometry.size.width - width
                        let position = phase < 0.5 ? phase * 2 : (1 - phase) * 2
                        Capsule()
                            .fill(Color.accentColor)
                            .frame(width: width)
                            .offset(x: travel * position)
                    }
                }
            }
        }
        .frame(height: 6)
    }
}

/// Schalter „Zeitmarken“; gilt für YouTube- und Datei-Transkripte und
/// sofort (gespeichert in settings.toml).
struct TimestampsRow: View {
    @ObservedObject var model: Model

    var body: some View {
        Row(symbol: "clock", title: model.t("Zeitmarken", "Timestamps"),
            subtitle: model.t("[12:30] vor jedem Absatz", "[12:30] before each paragraph")) {
            Toggle("", isOn: Binding(
                get: { model.settings.entries.timestamps },
                set: { model.settings.entries.timestamps = $0; model.saveSettings() }))
                .toggleStyle(.switch).labelsHidden()
        }
    }
}

/// Schalter „Kapitelüberschriften“; gilt für YouTube-Transkripte (Dateien
/// haben keine Kapitel) und sofort (gespeichert in settings.toml).
struct ChaptersRow: View {
    @ObservedObject var model: Model

    var body: some View {
        Row(symbol: "list.bullet.indent", title: model.t("Kapitelüberschriften", "Chapter headings"),
            subtitle: model.t("Kapitel des Videos als Überschriften", "The video's chapters as headings")) {
            Toggle("", isOn: Binding(
                get: { model.settings.entries.chapters },
                set: { model.settings.entries.chapters = $0; model.saveSettings() }))
                .toggleStyle(.switch).labelsHidden()
        }
    }
}

/// Felder „von“ und „bis“: nur diesen Teil transkribieren; leer = alles.
struct RangeFields: View {
    @ObservedObject var model: Model
    @Binding var from: String
    @Binding var to: String
    /// „das ganze Video“ oder „die ganze Datei“.
    let whole: String
    /// Warum der Bereich gerade nicht geht (dann ausgegraut).
    var unavailable: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Row(symbol: "scissors", title: model.t("Nur einen Teil transkribieren", "Transcribe only a part"),
                subtitle: unavailable ?? model.t("Felder leer lassen für \(whole).",
                                                 "Leave the fields empty for \(whole).")) { EmptyView() }
            HStack(spacing: 8) {
                Text(model.t("von", "from")).fixedSize()
                TextField("0:00", text: $from).frame(width: 72)
                Text(model.t("bis", "to")).fixedSize()
                TextField(model.t("Ende", "End"), text: $to).frame(width: 72)
                Spacer(minLength: 0)
                Button {
                    from = ""
                    to = ""
                } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help(model.t("Felder leeren", "Clear fields"))
                .opacity(from.isEmpty && to.isEmpty ? 0 : 1)
            }
            .font(.callout.monospacedDigit())
            .foregroundStyle(.secondary)
            .textFieldStyle(.roundedBorder)
            .multilineTextAlignment(.center)
            .padding(.leading, 46)
            .padding(.trailing, 12)
            .padding(.bottom, 10)
        }
        .disabled(unavailable != nil)
        .opacity(unavailable == nil ? 1 : 0.4)
    }
}

/// Block „Transkript anpassen“: Teil des Videos bzw. der Datei, bei Videos
/// die Kapitelüberschriften, und Zeitmarken.
struct TranscriptOptions: View {
    @ObservedObject var model: Model
    let source: SourceKind
    @Binding var from: String
    @Binding var to: String
    let whole: String
    var unavailable: String?

    var body: some View {
        SectionTitle(title: model.t("Transkript anpassen", "Adjust transcript"))
        Card {
            RangeFields(model: model, from: $from, to: $to, whole: whole, unavailable: unavailable)
            switch source {
            case .youtube:
                RowDivider()
                ChaptersRow(model: model)
            case .file:
                EmptyView()
            }
            RowDivider()
            TimestampsRow(model: model)
        }
    }
}

/// Anklickbare Zeile mit einer Sprachauswahl darunter.
struct LanguageActionRow: View {
    let symbol: String
    let title: String
    let subtitle: String
    /// Beschriftung vor der Auswahl („Sprache“, „Zielsprache“).
    let label: String
    @Binding var selection: String
    let options: [LanguageOption]
    /// Text eines Eintrags („Deutsch“, „ins Deutsche“).
    let optionText: (LanguageOption) -> String
    let action: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ActionRow(symbol: symbol, title: title, subtitle: subtitle, action: action)
            HStack(spacing: 8) {
                Text(label).font(.callout).foregroundStyle(.secondary)
                Picker("", selection: $selection) {
                    ForEach(options, id: \.code) { option in
                        Text(optionText(option)).tag(option.code)
                    }
                }
                .labelsHidden()
                .pickerStyle(.menu)
                .fixedSize()
                Spacer(minLength: 0)
            }
            .padding(.leading, 46)
            .padding(.trailing, 12)
            .padding(.bottom, 10)
        }
    }
}

/// Zeile „Letztes Transkript kopieren“ in beiden Reitern.
struct CopyLastRow: View {
    @ObservedObject var model: Model
    let title: String
    let source: SourceKind

    var body: some View {
        Card {
            ActionRow(symbol: "doc.on.clipboard", title: title,
                      subtitle: model.t("Falls du nach der letzten Transkription\nbereits etwas anderes kopiert hast.",
                                        "In case you have copied something\nelse since the last transcription.")) {
                send("copy_last_transcript", ["source": source.rawValue])
            }
        }
    }
}

struct YoutubeTab: View {
    @ObservedObject var model: Model

    var idle: Bool { model.transcript.state == .idle }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionTitle(title: "YouTube")
            Hint(text: model.t("Link im Browser kopieren, dann hier wählen.\nDas Transkript landet in der Zwischenablage.",
                               "Copy the link in the browser, then choose here.\nThe transcript ends up on the clipboard."))
            Group {
                TranscriptOptions(model: model, source: .youtube, from: $model.youtubeFrom, to: $model.youtubeTo,
                                  whole: model.t("das ganze Video", "the whole video"))
                SectionTitle(title: model.t("YouTube-Video transkribieren", "Transcribe YouTube video"))
                Card {
                    ActionRow(symbol: "play.rectangle", title: model.t("YouTube-Video", "YouTube video"),
                              subtitle: model.t("Die Sprache, die YouTube für das Video\nangibt, wird übernommen.",
                                                "Uses the language YouTube\nstates for the video.")) {
                        model.transcribeYoutube(language: NSNull())
                    }
                    .modifier(LinkCheck(model: model, row: "automatic"))
                    RowDivider()
                    LanguageActionRow(
                        symbol: "play.rectangle", title: model.t("Sprache selbst wählen", "Choose the language"),
                        subtitle: model.t("Falls YouTube keine oder die falsche\nSprache angibt.",
                                          "If YouTube states no language\nor the wrong one."),
                        label: model.t("Sprache", "Language"), selection: $model.youtubeLanguage,
                        options: model.transcriptionLanguages, optionText: model.name(of:)
                    ) {
                        model.transcribeYoutube(language: model.youtubeLanguage)
                    }
                    .modifier(LinkCheck(model: model, row: "chosen"))
                }
                SectionTitle(title: model.t("YouTube-Video übersetzen", "Translate YouTube video"))
                Card {
                    LanguageActionRow(
                        symbol: "character.book.closed",
                        title: model.t("Transkribieren und übersetzen", "Transcribe and translate"),
                        subtitle: model.t("Wird erst in der Originalsprache transkribiert,\ndann vom Sprachmodell übersetzt.",
                                          "Transcribed in the original language first,\nthen translated by the language model."),
                        label: model.t("Zielsprache", "Target language"), selection: $model.translateTarget,
                        options: model.transcriptionLanguages,
                        optionText: { model.english ? "into \($0.english)" : "ins \($0.name)e" }
                    ) {
                        model.transcribeYoutube(language: NSNull(), translate: model.translateTarget)
                    }
                    .modifier(LinkCheck(model: model, row: "translate"))
                }
            }
            .disabled(!idle)
            .opacity(idle ? 1 : 0.4)
            TranscriptProgress(model: model)
            CopyLastRow(model: model, title: model.t("Letztes YT-Transkript kopieren", "Copy last YT transcript"),
                        source: .youtube)
        }
    }
}

struct FileTab: View {
    @ObservedObject var model: Model

    var idle: Bool { model.transcript.state == .idle }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionTitle(title: model.t("Audio- und Videodateien", "Audio and video files"))
            DropZone(model: model)
            Hint(text: model.t("Auch aufs Menüleisten-Symbol ziehen oder im Finder\nmarkieren und ⌘C drücken. Das Transkript landet in\nder Zwischenablage und als .txt neben der Datei.",
                               "You can also drag onto the menu bar icon, or select\nin the Finder and press ⌘C. The transcript ends up on\nthe clipboard and as a .txt next to the file."))
            TranscriptOptions(model: model, source: .file, from: $model.fileFrom, to: $model.fileTo,
                              whole: model.t("die ganze Datei", "the whole file"),
                              unavailable: model.droppedFiles.count > 1
                                  ? model.t("Geht nur bei einer einzelnen Datei.", "Only works with a single file.") : nil)
            SectionTitle(title: model.t("Datei transkribieren", "Transcribe file"))
            Card {
                LanguageActionRow(
                    symbol: "waveform", title: model.t("Datei transkribieren", "Transcribe file"),
                    subtitle: model.t("Ohne gezogene Datei wird die im Finder\nkopierte genommen.",
                                      "Without a dropped file, the one copied\nin the Finder is used."),
                    label: model.t("Sprache", "Language"), selection: $model.fileLanguage,
                    options: model.transcriptionLanguages, optionText: model.name(of:)
                ) {
                    model.transcribeFiles(language: model.fileLanguage)
                }
                .background(model.startTargeted ? Color.accentColor.opacity(0.14) : .clear)
                // Direkt hierher gezogen, geht es sofort los.
                .dropDestination(for: URL.self) { urls, _ in
                    let files = urls.filter(\.isFileURL)
                    guard !files.isEmpty else { return false }
                    model.transcribeFiles(language: model.fileLanguage, dropped: files)
                    return true
                } isTargeted: { model.startTargeted = $0 }
            }
            .overlay(RoundedRectangle(cornerRadius: 12)
                .stroke(Color.accentColor, lineWidth: model.startTargeted ? 2 : 0))
            .animation(.easeOut(duration: 0.15), value: model.startTargeted)
            TranscriptProgress(model: model)
            SectionTitle(title: model.t("Formate", "Formats"))
            Card {
                Row(symbol: "music.note", title: "Audio",
                    subtitle: "MP3, M4A, AAC, WAV, AIFF,\nFLAC, OGG, Opus, WMA") { EmptyView() }
                RowDivider()
                Row(symbol: "film", title: model.t("Video (die Tonspur zählt)", "Video (the audio track counts)"),
                    subtitle: "MP4, MOV, M4V, MKV, WebM, AVI") { EmptyView() }
            }
            Hint(text: model.t("Dazu alles, was ffmpeg lesen kann. Die Länge ist beliebig.",
                               "Plus anything ffmpeg can read. Any length."))
            CopyLastRow(model: model, title: model.t("Letztes Datei-Transkript kopieren", "Copy last file transcript"),
                        source: .file)
        }
    }
}

/// Große Ablagefläche für Dateien: gestrichelt, solange sie leer ist; beim
/// Drüberziehen färbt sie sich und das Symbol springt; danach zeigt sie,
/// welche Dateien auf den Start warten. Mehrere werden nacheinander
/// transkribiert.
struct DropZone: View {
    @ObservedObject var model: Model

    var targeted: Bool { model.dropTargeted }
    var files: [URL] { model.droppedFiles }

    var body: some View {
        ZStack(alignment: .topTrailing) {
            VStack(spacing: 6) {
                Image(systemName: symbol)
                    .font(.system(size: 30, weight: .regular))
                    .foregroundStyle(targeted || !files.isEmpty ? Color.accentColor : .secondary)
                    .scaleEffect(targeted ? 1.15 : 1)
                Text(title)
                    .font(.body.weight(.semibold))
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
            }
            .padding(.horizontal, 16)
            .frame(maxWidth: .infinity, minHeight: 128)
            if !files.isEmpty && !targeted {
                Button {
                    model.droppedFiles = []
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 16))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help(model.t("Datei verwerfen", "Discard file"))
                .padding(10)
            }
        }
        .background(RoundedRectangle(cornerRadius: 12).fill(background))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(border, style: StrokeStyle(
            lineWidth: targeted ? 2 : 1.5, dash: files.isEmpty || targeted ? [6, 4] : [])))
        .contentShape(RoundedRectangle(cornerRadius: 12))
        .animation(.spring(response: 0.25, dampingFraction: 0.7), value: targeted)
        .animation(.easeOut(duration: 0.2), value: files)
        .dropDestination(for: URL.self) { urls, _ in
            let dropped = urls.filter(\.isFileURL)
            guard !dropped.isEmpty else { return false }
            model.droppedFiles = dropped
            return true
        } isTargeted: { model.dropTargeted = $0 }
    }

    var symbol: String {
        if targeted { return "arrow.down.circle.fill" }
        if files.isEmpty { return "tray.and.arrow.down" }
        return files.count == 1 ? "doc.fill" : "doc.on.doc.fill"
    }

    var title: String {
        if targeted { return model.t("Loslassen zum Übernehmen", "Release to add") }
        switch files.count {
        case 0: return model.t("Dateien hierher ziehen", "Drop files here")
        case 1: return files[0].lastPathComponent
        default: return model.t("\(files.count) Dateien", "\(files.count) files")
        }
    }

    var subtitle: String {
        if files.isEmpty || targeted {
            return model.t("Mehrere werden nacheinander transkribiert.",
                           "Several are transcribed one after another.")
        }
        return model.t("Sprache wählen und unten starten.", "Choose the language and start below.")
    }

    var background: Color {
        if targeted { return Color.accentColor.opacity(0.14) }
        return files.isEmpty ? Color.primary.opacity(0.03) : Color.accentColor.opacity(0.07)
    }

    var border: Color {
        if targeted { return Color.accentColor }
        return files.isEmpty ? Color.primary.opacity(0.25) : Color.accentColor.opacity(0.5)
    }
}
