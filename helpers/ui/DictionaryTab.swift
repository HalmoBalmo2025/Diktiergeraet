// Reiter „Wörterbuch“: Jura-Regeln, Ersetzungen, Fachbegriffe, Gesetzeskürzel.

import AppKit
import SwiftUI

struct DictionaryTab: View {
    @ObservedObject var model: Model

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionTitle(title: model.t("Jura-Regeln", "Legal rules"))
            Card {
                Row(symbol: "text.book.closed", title: model.t("Normzitate umformen", "Reformat statute citations"),
                    subtitle: "Paragraf 433 Absatz 2 → § 433 II") {
                    Toggle("", isOn: Binding(
                        get: { model.dictionary.legal },
                        set: { model.dictionary.legal = $0; model.saveDictionary() }))
                        .toggleStyle(.switch).labelsHidden()
                }
                RowDivider()
                Row(symbol: "textformat", title: model.t("Absätze", "Subsections")) {
                    Picker("", selection: Binding(
                        get: { model.dictionary.style },
                        set: { model.dictionary.style = $0; model.saveDictionary() })) {
                        Text("§ 433 II").tag("römisch")
                        Text("Abs. 2").tag("abgekürzt")
                    }
                    .pickerStyle(.segmented).labelsHidden().fixedSize()
                }
                .disabled(!model.dictionary.legal)
            }

            SectionTitle(title: model.t("Ersetzungen", "Replacements"))
            Card {
                let replacements = model.dictionary.replacements
                if !replacements.isEmpty {
                    ScrollView {
                        VStack(spacing: 0) {
                            ForEach(Array(replacements.enumerated()), id: \.offset) { index, entry in
                                if index > 0 { RowDivider() }
                                ReplacementRow(model: model, entry: entry) {
                                    model.dictionary.replacements.remove(at: index)
                                    model.saveDictionary()
                                }
                            }
                        }
                    }
                    .frame(height: min(CGFloat(replacements.count) * 38, 190))
                    RowDivider()
                }
                HStack(spacing: 8) {
                    TextField(model.t("Modell schreibt", "Model writes"), text: $model.newFrom)
                    Image(systemName: "arrow.right").foregroundStyle(.tertiary)
                    TextField(model.t("Einfügen", "Insert"), text: $model.newTo).onSubmit(addReplacement)
                    Button(action: addReplacement) {
                        Image(systemName: "plus.circle.fill").font(.system(size: 18))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(Color.accentColor)
                    .disabled(model.newFrom.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .textFieldStyle(.roundedBorder)
                .padding(10)
            }

            SectionTitle(title: model.t("Fachbegriffe", "Technical terms"))
            Hint(text: model.t("Ähnlich erkannte Wörter werden zum Begriff:\n„Subsomo“ → „Subsumo“ (ab 5 Buchstaben).",
                               "Similar recognised words become the term:\n“Subsomo” → “Subsumo” (5 letters or more)."))
            Card {
                if !model.dictionary.terms.isEmpty {
                    FlowChips(items: model.dictionary.terms, minimum: 140) { index in
                        model.dictionary.terms.remove(at: index)
                        model.saveDictionary()
                    }
                    .padding(10)
                    RowDivider()
                }
                HStack(spacing: 8) {
                    TextField(model.t("z. B. Subsumo", "e.g. Subsumo"), text: $model.newTerm).onSubmit(addTerm)
                    Button(action: addTerm) {
                        Image(systemName: "plus.circle.fill").font(.system(size: 18))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(Color.accentColor)
                    .disabled(model.newTerm.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .textFieldStyle(.roundedBorder)
                .padding(10)
            }

            SectionTitle(title: model.t("Weitere Gesetzeskürzel", "More statute abbreviations"))
            Card {
                if !model.dictionary.statutes.isEmpty {
                    FlowChips(items: model.dictionary.statutes) { index in
                        model.dictionary.statutes.remove(at: index)
                        model.saveDictionary()
                    }
                    .padding(10)
                    RowDivider()
                }
                HStack(spacing: 8) {
                    TextField(model.t("z. B. KAGB", "e.g. KAGB"), text: $model.newStatute).onSubmit(addStatute)
                    Button(action: addStatute) {
                        Image(systemName: "plus.circle.fill").font(.system(size: 18))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(Color.accentColor)
                    .disabled(model.newStatute.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .textFieldStyle(.roundedBorder)
                .padding(10)
            }
            FileButton(title: model.t("Datei öffnen", "Open file"), command: "open_dictionary")
        }
    }

    func addReplacement() {
        let from = model.newFrom.trimmingCharacters(in: .whitespaces)
        guard !from.isEmpty else { return }
        model.dictionary.replacements.removeAll { $0.from == from }
        model.dictionary.replacements.append(
            ReplacementEntry(from: from, to: model.newTo.trimmingCharacters(in: .whitespaces)))
        model.saveDictionary()
        model.newFrom = ""
        model.newTo = ""
    }

    func addTerm() {
        let term = model.newTerm.trimmingCharacters(in: .whitespaces)
        guard !term.isEmpty, !term.contains(" "), !model.dictionary.terms.contains(term) else { return }
        model.dictionary.terms.append(term)
        model.saveDictionary()
        model.newTerm = ""
    }

    func addStatute() {
        let statute = model.newStatute.trimmingCharacters(in: .whitespaces)
        guard !statute.isEmpty, !model.dictionary.statutes.contains(statute) else { return }
        model.dictionary.statutes.append(statute)
        model.saveDictionary()
        model.newStatute = ""
    }
}

struct ReplacementRow: View {
    @ObservedObject var model: Model
    let entry: ReplacementEntry
    let delete: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Text(entry.from).lineLimit(1)
            Image(systemName: "arrow.right").font(.caption).foregroundStyle(.tertiary)
            Text(entry.to.isEmpty ? model.t("(entfernen)", "(remove)") : entry.to)
                .lineLimit(1)
                .foregroundStyle(entry.to.isEmpty ? .secondary : .primary)
            Spacer(minLength: 4)
            Button(action: delete) {
                Image(systemName: "minus.circle").foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help(model.t("Ersetzung löschen", "Delete replacement"))
        }
        .padding(.horizontal, 12)
        .frame(height: 38)
    }
}

/// Kürzel als Schildchen mit ×, umgebrochen auf mehrere Zeilen.
struct FlowChips: View {
    let items: [String]
    /// Breite einer Spalte; längere Wörter brauchen mehr.
    var minimum: CGFloat = 70
    let delete: (Int) -> Void

    var body: some View {
        let columns = [GridItem(.adaptive(minimum: minimum), spacing: 6, alignment: .leading)]
        LazyVGrid(columns: columns, alignment: .leading, spacing: 6) {
            ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                HStack(spacing: 4) {
                    Text(item).font(.caption.weight(.semibold)).lineLimit(1)
                    Button { delete(index) } label: {
                        Image(systemName: "xmark").font(.system(size: 9, weight: .bold))
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Capsule().fill(Color.primary.opacity(0.08)))
            }
        }
    }
}

/// Grauer Knopf „Datei öffnen“ über die ganze Breite (für alle, die lieber
/// die Datei bearbeiten).
struct FileButton: View {
    let title: String
    let command: String

    var body: some View {
        Button {
            send(command)
        } label: {
            Label(title, systemImage: "doc.text")
                .font(.callout.weight(.medium))
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, minHeight: 32)
                .background(RoundedRectangle(cornerRadius: 10).fill(Color.primary.opacity(0.05)))
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color.primary.opacity(0.08)))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
