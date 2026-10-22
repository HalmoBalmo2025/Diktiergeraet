// diktator-spell: prüft Wörter mit der Rechtschreibprüfung von macOS.
//
//   printf 'Hause\ndan\n' | diktator-spell de
//
// Liest ein Wort pro Zeile und schreibt für jedes eine Zeile: 1 = richtig
// geschrieben, 0 = unbekannt oder falsch. Rückgabewert 1 (Grund auf stderr,
// auf Deutsch und Englisch),
// wenn macOS die Sprache nicht prüfen kann. Das Framework gibt es nur für
// Swift/Objective-C; so braucht Diktator selbst kein `unsafe`.

import AppKit

/// Meldet den Grund auf Deutsch und Englisch als JSON-Zeile auf stderr
/// (Diktator liest sie als `Note`, siehe src/helper.rs) und endet.
func fail(_ german: String, _ english: String) -> Never {
    let data = (try? JSONSerialization.data(withJSONObject: ["de": german, "en": english])) ?? Data(german.utf8)
    FileHandle.standardError.write(data)
    exit(1)
}

let arguments = CommandLine.arguments
guard arguments.count == 2 else { fail("Aufruf: diktator-spell <Sprachcode> < Wörter", "Usage: diktator-spell <language code> < words") }
let checker = NSSpellChecker.shared
let wanted = arguments[1]
// „pt“ gibt es nur als „pt_BR“ und „pt_PT“.
guard let language = checker.availableLanguages.first(where: { $0 == wanted })
    ?? checker.availableLanguages.first(where: { $0.hasPrefix(wanted + "_") })
else { fail("Rechtschreibprüfung für „\(wanted)“ nicht verfügbar", "Spell checking for “\(wanted)” not available") }

let input = String(decoding: FileHandle.standardInput.readDataToEndOfFile(), as: UTF8.self)
var output = ""
for word in input.split(separator: "\n", omittingEmptySubsequences: false) {
    let range = checker.checkSpelling(
        of: String(word), startingAt: 0, language: language, wrap: false,
        inSpellDocumentWithTag: 0, wordCount: nil)
    output += range.location == NSNotFound && !word.isEmpty ? "1\n" : "0\n"
}
FileHandle.standardOutput.write(Data(output.utf8))
