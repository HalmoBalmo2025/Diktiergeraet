// Das Panel: Kopf mit Zustand und Meldung, Reiterleiste, Inhalt des Reiters.

import AppKit
import SwiftUI

struct Panel: View {
    @ObservedObject var model: Model

    var body: some View {
        VStack(spacing: 14) {
            header
            TabBar(model: model)
            content
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(16)
        // Deckender Hintergrund statt durchscheinendem Glas: gut lesbar.
        .background(Color(nsColor: .windowBackgroundColor).ignoresSafeArea())
    }

    /// Symbol links, Zustand und letzte Meldung rechts daneben. Die Höhe
    /// ist fest (Platz für zwei Zeilen Meldung), damit das Panel nicht
    /// springt, wenn eine Meldung kommt oder geht.
    var header: some View {
        HStack(alignment: .center, spacing: 12) {
            Image(systemName: "waveform.circle.fill")
                .font(.system(size: 44, weight: .regular))
                .foregroundStyle(.primary)
            // Gleich viel Luft über dem Zustand, zwischen Zustand und Meldung
            // und darunter; ohne Meldung steht der Zustand in der Mitte. Die
            // Höhe ist fest, damit das Panel nicht springt.
            VStack(alignment: .leading, spacing: 0) {
                Spacer(minLength: 0)
                HStack(spacing: 6) {
                    Circle().fill(model.statusColor).frame(width: 7, height: 7)
                    Text(model.statusText).font(.headline)
                }
                if !model.noteText.isEmpty {
                    Spacer(minLength: 0)
                    Text(model.noteText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                        // Passt die Meldung nicht in zwei Zeilen, steht sie
                        // ganz im Tooltip.
                        .help(model.noteText)
                }
                Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity, minHeight: 56, maxHeight: 56, alignment: .leading)
        }
        .padding(.horizontal, 4)
    }


    @ViewBuilder var content: some View {
        switch model.tab {
        case .dictation: DictationTab(model: model)
        case .youtube: YoutubeTab(model: model)
        case .file: FileTab(model: model)
        case .dictionary: DictionaryTab(model: model)
        case .settings: SettingsTab(model: model)
        case .info: InfoTab(model: model)
        }
    }

}

struct TabBar: View {
    @ObservedObject var model: Model
    var selection: Tab { model.tab }

    var body: some View {
        HStack(spacing: 4) {
            ForEach(Tab.allCases) { tab in
                Button {
                    model.tab = tab
                } label: {
                    Image(systemName: tab.symbol)
                        .font(.system(size: 16, weight: .regular))
                        .frame(maxWidth: .infinity, minHeight: 34)
                        .foregroundStyle(selection == tab ? Color.accentColor : .secondary)
                        .background(
                            RoundedRectangle(cornerRadius: 9)
                                .fill(selection == tab ? Color.accentColor.opacity(0.15) : .clear))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(tab.title(model))
            }
            Divider().frame(height: 22).padding(.horizontal, 2)
            Button {
                send("quit")
            } label: {
                Image(systemName: "power")
                    .font(.system(size: 16, weight: .regular))
                    .frame(maxWidth: .infinity, minHeight: 34)
                    .foregroundStyle(.secondary)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(model.t("Diktator beenden", "Quit Diktator"))
        }
        .padding(5)
        .background(RoundedRectangle(cornerRadius: 13).fill(Color.primary.opacity(0.05)))
        .overlay(RoundedRectangle(cornerRadius: 13).stroke(Color.primary.opacity(0.06)))
    }
}
