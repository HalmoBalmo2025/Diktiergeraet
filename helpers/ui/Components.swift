// Bausteine der Reiter: Überschriften, Karten, Zeilen, Tasten, Hinweise.

import AppKit
import SwiftUI

struct SectionTitle: View {
    let title: String

    var body: some View {
        HStack {
            Text(title.uppercased())
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding(.horizontal, 2)
    }
}

struct Card<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(spacing: 0) { content }
            .background(RoundedRectangle(cornerRadius: 12).fill(Color.primary.opacity(0.05)))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color.primary.opacity(0.06)))
    }
}

struct RowDivider: View {
    var body: some View { Divider().padding(.leading, 46) }
}

/// Eine Zeile mit Symbol, Titel, optionaler Unterzeile und Inhalt rechts.
struct Row<Trailing: View>: View {
    let symbol: String
    let title: String
    var subtitle: String?
    /// Unterzeile mit Hervorhebungen (statt `subtitle`).
    var detail: AttributedString?
    @ViewBuilder let trailing: Trailing

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: symbol)
                .font(.system(size: 16))
                .foregroundStyle(Color.accentColor)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.body.weight(.medium)).lineLimit(2)
                if let subtitle {
                    Text(subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
                if let detail {
                    Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
            }
            Spacer(minLength: 8)
            trailing
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .contentShape(Rectangle())
    }
}

/// Eine anklickbare Zeile mit Pfeil rechts.
struct ActionRow: View {
    let symbol: String
    let title: String
    var subtitle: String?
    var trailingSymbol = "chevron.right"
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Row(symbol: symbol, title: title, subtitle: subtitle) {
                Image(systemName: trailingSymbol)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
        }
        .buttonStyle(.plain)
    }
}

struct Keycap: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.caption.weight(.semibold))
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(RoundedRectangle(cornerRadius: 5).fill(Color.primary.opacity(0.08)))
    }
}

struct Hint: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 2)
    }
}

/// Färbt eine YouTube-Zeile, solange die Maus darauf liegt: grün, wenn ein
/// YouTube-Link in der Zwischenablage liegt, sonst rot. Diktator prüft nur
/// die Form des Links (`check_link`), nicht, ob das Video abrufbar ist.
struct LinkCheck: ViewModifier {
    @ObservedObject var model: Model
    /// Name der Zeile, eindeutig im Reiter.
    let row: String

    var tint: Color? {
        guard model.hoveredLinkRow == row, model.transcript.state == .idle else { return nil }
        return switch model.clipboardLink {
        case .youtube: .green
        case .missing, .unreadable: .red
        case nil: nil
        }
    }

    var hint: String {
        switch model.clipboardLink {
        case .youtube: model.t("YouTube-Link in der Zwischenablage", "YouTube link on the clipboard")
        case .missing: model.t("Kein YouTube-Link in der Zwischenablage", "No YouTube link on the clipboard")
        case .unreadable: model.t("Zwischenablage nicht lesbar", "Clipboard not readable")
        case nil: ""
        }
    }

    func body(content: Content) -> some View {
        content
            .background(tint.map { $0.opacity(0.15) } ?? Color.clear)
            .animation(.easeOut(duration: 0.12), value: tint)
            .onHover { model.hoverLinkRow(row, inside: $0) }
            .help(hint)
    }
}
