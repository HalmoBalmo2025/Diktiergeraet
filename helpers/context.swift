// diktator-context: gibt den Text links und rechts der Schreibmarke im
// fokussierten Textfeld aus, getrennt durch ein NUL-Zeichen.
//
// Diktator ruft das vor jedem Einfügen auf, um Groß-/Kleinschreibung,
// Leerzeichen und Schlusspunkt an die Umgebung anzupassen. Die Bedienungs-
// hilfen-API gibt es bequem nur in Swift; so braucht Diktator selbst kein
// `unsafe`.
//
//   diktator-context            vordere App
//   diktator-context <pid>      bestimmte App (zum Testen)
//
// Ausgabe (Rückgabewert 0), Felder getrennt durch NUL:
//   „app\0links\0rechts“   lesbar (je höchstens 40 Zeichen)
//   „app\0Grund“            App bekannt, Text um die Schreibmarke nicht
// `app` ist die Bundle-ID der App (für die Regeln je App in Diktator).
// Ist nicht einmal die App bekannt: Rückgabewert 1, Grund auf stderr.

import AppKit
import ApplicationServices

/// So viele Zeichen werden auf jeder Seite gelesen.
let window = 40

/// Wie lange auf eine antwortunwillige App je Abfrage gewartet wird
/// (Sekunden).
let timeout: Float = 0.4

/// Alle Abfragen zusammen enden vor der Zeitgrenze von Diktator (800 ms),
/// damit wenigstens die App rechtzeitig ankommt.
let deadline = Date().addingTimeInterval(0.6)

/// Setzt die Wartezeit der nächsten Abfrage an `element` auf das, was vom
/// Zeitbudget übrig ist (höchstens `timeout`).
func budget(_ element: AXUIElement) {
    AXUIElementSetMessagingTimeout(element, min(timeout, Float(max(0.01, deadline.timeIntervalSinceNow))))
}

/// Meldet den Grund auf Deutsch und Englisch als JSON-Zeile auf stderr
/// (Diktator liest sie als `Note`, siehe src/helper.rs) und endet.
func fail(_ german: String, _ english: String) -> Never {
    let data = (try? JSONSerialization.data(withJSONObject: ["de": german, "en": english])) ?? Data(german.utf8)
    FileHandle.standardError.write(data)
    exit(1)
}

/// Bundle-ID der App, in die eingefügt wird (leer, wenn unbekannt).
var bundle = ""

/// Die App ist bekannt, der Text um die Schreibmarke nicht.
func unreadable(_ reason: String) -> Never {
    FileHandle.standardOutput.write(Data((bundle + "\u{0}" + reason).utf8))
    exit(0)
}

func attribute(_ element: AXUIElement, _ name: String) -> (CFTypeRef?, AXError) {
    var value: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    return (status == .success ? value : nil, status)
}

/// Text im Bereich, bevorzugt über AXStringForRange (liest nicht das ganze
/// Dokument); nur wenn die App das nicht kann, aus dem vollständigen Wert.
func text(_ element: AXUIElement, _ range: CFRange, fullValue: () -> String?) -> String? {
    var cfRange = range
    if let rangeValue = AXValueCreate(.cfRange, &cfRange) {
        var result: CFTypeRef?
        budget(element)
        let status = AXUIElementCopyParameterizedAttributeValue(
            element, kAXStringForRangeParameterizedAttribute as CFString, rangeValue, &result)
        if status == .success, let string = result as? String {
            return string
        }
    }
    guard let full = fullValue() as NSString? else { return nil }
    let start = max(0, min(range.location, full.length))
    let length = max(0, min(range.length, full.length - start))
    return full.substring(with: NSRange(location: start, length: length))
}

let pid: pid_t
if let argument = CommandLine.arguments.dropFirst().first, let given = pid_t(argument) {
    pid = given
} else if let front = NSWorkspace.shared.frontmostApplication {
    pid = front.processIdentifier
} else {
    fail("keine vordere App", "no front app")
}
bundle = NSRunningApplication(processIdentifier: pid)?.bundleIdentifier ?? ""
guard AXIsProcessTrusted() else { unreadable("kein Recht „Bedienungshilfen“") }
// Terminals liefern den ganzen Bildschirm (Rahmenlinien, leere Zellen als
// Nullzeichen) und keine echte Schreibmarke: dort nichts lesen, Diktator
// nimmt dann das Ende des zuletzt eingefügten Teils.
let terminals: Set<String> = [
    "com.apple.Terminal", "com.googlecode.iterm2", "dev.warp.Warp-Stable", "com.mitchellh.ghostty",
    "net.kovidgoyal.kitty", "org.alacritty", "io.alacritty", "com.github.wez.wezterm",
]
if terminals.contains(bundle) {
    unreadable("Terminal (\(bundle)): Umgebung wird nicht gelesen")
}

let app = AXUIElementCreateApplication(pid)
budget(app)
let appName = NSRunningApplication(processIdentifier: pid)?.localizedName ?? "pid \(pid)"

// Manche Apps geben ihr fokussiertes Element erst preis, wenn man sie als
// Bedienungshilfe anspricht (Chrome, Electron-Apps wie Claude).
AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)

budget(app)
let (focusedRef, focusedStatus) = attribute(app, kAXFocusedUIElementAttribute)
guard let focusedRef, CFGetTypeID(focusedRef) == AXUIElementGetTypeID() else {
    unreadable("\(appName): kein fokussiertes Element (AXError \(focusedStatus.rawValue))")
}
let focused = focusedRef as! AXUIElement
budget(focused)
let role = attribute(focused, kAXRoleAttribute).0 as? String ?? "?"

budget(focused)
let (rangeRef, rangeStatus) = attribute(focused, kAXSelectedTextRangeAttribute)
guard let rangeRef, CFGetTypeID(rangeRef) == AXValueGetTypeID() else {
    unreadable("\(appName): \(role) ohne Schreibmarke (AXError \(rangeStatus.rawValue))")
}
var selection = CFRange()
guard AXValueGetValue(rangeRef as! AXValue, .cfRange, &selection) else {
    unreadable("\(appName): Schreibmarke unlesbar")
}

/// Der ganze Text des Felds, nur bei Bedarf und höchstens einmal gelesen
/// (in großen Dokumenten langsam).
var fullValueRead: String??
func fullValue() -> String? {
    if let read = fullValueRead { return read }
    budget(focused)
    let value = attribute(focused, kAXValueAttribute).0 as? String
    fullValueRead = .some(value)
    return value
}

// Die Länge ist billig zu haben; mit ihr fragt der Bereich rechts nicht über
// das Ende hinaus (daran scheitert AXStringForRange in manchen Apps).
budget(focused)
let length = attribute(focused, kAXNumberOfCharactersAttribute).0 as? Int
let beforeStart = max(0, selection.location - window)
let before = text(
    focused, CFRange(location: beforeStart, length: selection.location - beforeStart),
    fullValue: fullValue)
let afterStart = selection.location + selection.length
let afterLength = length.map { max(0, min(window, $0 - afterStart)) } ?? window
let after = afterLength == 0
    ? ""
    : text(focused, CFRange(location: afterStart, length: afterLength), fullValue: fullValue)

guard let before else { unreadable("\(appName): \(role) verrät den Text nicht") }
// Das Nullzeichen trennt links und rechts; im Text selbst wird es zum Leerzeichen.
func clean(_ text: String) -> String { text.replacingOccurrences(of: "\u{0}", with: " ") }
FileHandle.standardOutput.write(
    Data((bundle + "\u{0}" + clean(before) + "\u{0}" + clean(after ?? "")).utf8))
