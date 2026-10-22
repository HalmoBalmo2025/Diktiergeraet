// diktator-llm: überarbeitet Text mit Apples Sprachmodell auf dem Gerät
// (Foundation Models, Apple Intelligence).
//
//   echo "Text" | diktator-llm "<Anweisung>"
//
// Rückgabewert 0: der überarbeitete Text auf stdout. Sonst: Grund auf stderr,
// auf Deutsch und Englisch
// (Apple Intelligence aus, Gerät nicht geeignet, Inhalt abgelehnt, Text zu
// lang …). Das Framework gibt es nur für Swift; so braucht Diktator selbst
// kein `unsafe`.

import Foundation
import FoundationModels

/// Meldet den Grund auf Deutsch und Englisch als JSON-Zeile auf stderr
/// (Diktator liest sie als `Note`, siehe src/helper.rs) und endet.
func fail(_ german: String, _ english: String) -> Never {
    let data = (try? JSONSerialization.data(withJSONObject: ["de": german, "en": english])) ?? Data(german.utf8)
    FileHandle.standardError.write(data)
    exit(1)
}

@main
enum Main {
    static func main() async {
        let arguments = CommandLine.arguments
        guard arguments.count == 2 else { fail("Aufruf: diktator-llm <Anweisung> < Text", "Usage: diktator-llm <instruction> < text") }
        let input = String(decoding: FileHandle.standardInput.readDataToEndOfFile(), as: UTF8.self)
        guard !input.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { fail("kein Text", "no text") }

        switch SystemLanguageModel.default.availability {
        case .available:
            break
        case .unavailable(.appleIntelligenceNotEnabled):
            fail("Apple Intelligence ist ausgeschaltet (Systemeinstellungen → Apple Intelligence & Siri)",
                 "Apple Intelligence is turned off (System Settings → Apple Intelligence & Siri)")
        case .unavailable(.deviceNotEligible):
            fail("Dieser Mac unterstützt Apple Intelligence nicht", "This Mac does not support Apple Intelligence")
        case .unavailable(.modelNotReady):
            fail("Apple Intelligence lädt das Modell noch herunter", "Apple Intelligence is still downloading the model")
        case .unavailable(let reason):
            fail("Apple Intelligence nicht verfügbar: \(reason)", "Apple Intelligence not available: \(reason)")
        }

        let session = LanguageModelSession(instructions: arguments[1])
        do {
            // Ohne Zufall: dieselbe Eingabe gibt dieselbe Überarbeitung.
            let response = try await session.respond(
                to: input, options: GenerationOptions(samplingMode: .greedy))
            print(response.content)
        } catch LanguageModelError.contextSizeExceeded {
            fail("Text zu lang für Apple Intelligence", "Text too long for Apple Intelligence")
        } catch LanguageModelError.guardrailViolation, LanguageModelError.refusal {
            fail("Apple Intelligence hat den Inhalt abgelehnt", "Apple Intelligence refused the content")
        } catch LanguageModelError.unsupportedLanguageOrLocale {
            fail("Apple Intelligence kann diese Sprache nicht", "Apple Intelligence does not support this language")
        } catch {
            fail("Apple Intelligence: \(error.localizedDescription)", "Apple Intelligence: \(error.localizedDescription)")
        }
    }
}
