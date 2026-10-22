// Reiter „Diktat“: Tasten und Verlauf.

import AppKit
import SwiftUI

struct DictationTab: View {
    @ObservedObject var model: Model

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionTitle(title: model.t("Diktat", "Dictation"))
            Card {
                Row(symbol: "mic", title: model.t("Deutsch", "German")) { Keycap(text: model.keyLabel) }
                RowDivider()
                Row(symbol: "sparkles", title: model.t("Deutsch überarbeiten", "German, polished")) {
                    Keycap(text: "\(model.keyLabel) ⌥")
                }
                RowDivider()
                Row(symbol: "globe", title: model.secondaryName) { Keycap(text: "\(model.keyLabel) ⇧") }
                RowDivider()
                Row(symbol: "sparkles",
                    title: model.t("\(model.secondaryName) überarbeiten", "\(model.secondaryName), polished")) {
                    Keycap(text: "\(model.keyLabel) ⇧ ⌥")
                }
                RowDivider()
                Row(symbol: "xmark.circle", title: model.t("Aufnahme verwerfen", "Discard recording")) {
                    Keycap(text: "Esc")
                }
            }
            SectionTitle(title: model.t("Verlauf", "History"))
            if model.history.isEmpty {
                Hint(text: model.t("Noch keine Diktate seit dem Start.", "No dictations since start."))
            } else {
                Card {
                    ForEach(Array(model.history.enumerated()), id: \.element.id) { index, entry in
                        if index > 0 { RowDivider() }
                        ActionRow(
                            symbol: "text.quote", title: entry.label,
                            trailingSymbol: "doc.on.doc"
                        ) {
                            send("copy_history", ["serial": entry.serial])
                        }
                        .onHover { model.hoverHistory(entry.serial, inside: $0) }
                        .popover(isPresented: Binding(
                            get: { model.shownHistory == entry.serial },
                            set: { if !$0 && model.shownHistory == entry.serial { model.shownHistory = nil } }
                        ), arrowEdge: .leading) {
                            FullText(text: entry.text)
                        }
                    }
                }
            }
        }
    }
}

/// Der ganze Text eines Diktats neben dem Verlauf; lange Texte scrollen.
struct FullText: View {
    let text: String

    /// Ab so vielen Zeichen bekommt die Blase eine feste Höhe und scrollt.
    static let scrollFrom = 900

    var body: some View {
        if text.count < FullText.scrollFrom {
            content
        } else {
            ScrollView { content }.frame(height: 420)
        }
    }

    var content: some View {
        Text(text)
            .font(.callout)
            .fixedSize(horizontal: false, vertical: true)
            .frame(width: 320, alignment: .leading)
            .padding(12)
    }
}
