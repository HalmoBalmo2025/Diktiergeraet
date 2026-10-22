// diktator-ui: Menüleisten-Symbol und Panel von Diktator.
//
// Diktator (Rust) startet dieses Programm und schickt jede Änderung als
// JSON-Zeile an die Standardeingabe, etwa
//   {"type":"status","data":{"state":"recording","language":"de"}}
// Befehle aus dem Panel gehen als JSON-Zeile an die Standardausgabe, etwa
//   {"command":"youtube","language":null}
// Endet die Standardeingabe (Diktator beendet), beendet sich auch dieses
// Programm. Ein NSPopover mit SwiftUI gibt es bequem nur in Swift; so
// braucht Diktator selbst kein `unsafe`.
//
// Der Quelltext ist auf die Dateien in diesem Ordner verteilt; `swiftc`
// übersetzt sie zusammen (siehe scripts/bundle.sh).

import AppKit
import SwiftUI

@main
enum Main {
    static func main() {
        let arguments = CommandLine.arguments
        if arguments.count == 3, arguments[1] == "--snapshot" {
            MainActor.assumeIsolated { snapshot(into: arguments[2]) }
            return
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let delegate = AppDelegate()
        app.delegate = delegate
        app.run()
    }
}
