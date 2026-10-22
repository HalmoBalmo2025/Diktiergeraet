// Menüleisten-Symbol, Panel (NSPopover) und Dateien, die aufs Symbol gezogen werden.

import AppKit
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSPopoverDelegate, NSWindowDelegate,
    NSDraggingDestination {
    let model = Model()
    var item: NSStatusItem?
    let popover = NSPopover()
    /// Beobachtet Klicks in anderen Apps und auf den Schreibtisch, solange
    /// das Panel offen ist. `.transient` allein schließt es bei einem Klick
    /// auf den leeren Schreibtisch nicht.
    var outsideClicks: Any?
    lazy var indicator = Indicator(model: model)

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Quadratisch wie die übrigen Symbole: gleichmäßige Abstände, und
        // der Pfeil des Panels bleibt an seinem Platz.
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.target = self
        item.button?.action = #selector(toggle)
        item.button?.imagePosition = .imageOnly
        // Eine Datei aufs Symbol ziehen öffnet das Panel im Reiter „Datei“.
        item.button?.window?.registerForDraggedTypes([.fileURL])
        item.button?.window?.delegate = self
        self.item = item

        popover.behavior = .transient
        popover.delegate = self
        popover.animates = true
        // Die Größe kommt aus dem SwiftUI-Inhalt; so wächst das Panel bei
        // einem anderen Reiter nach unten, verankert am Pfeil, statt nach
        // oben über die Menüleiste.
        let hosting = NSHostingController(rootView: Panel(model: model).frame(width: 360))
        hosting.sizingOptions = [.preferredContentSize]
        popover.contentViewController = hosting
        updateButton()
        startReading()
        // `--open [reiter]`: Panel gleich zeigen (zum Prüfen ohne Klicken).
        let arguments = CommandLine.arguments
        if let tab = arguments.compactMap(Tab.init(rawValue:)).first { model.tab = tab }
        if arguments.contains("--open") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { self.toggle() }
        }
    }

    @objc func toggle() {
        guard let button = item?.button else { return }
        if popover.isShown {
            popover.performClose(nil)
        } else {
            popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
            NSApp.activate(ignoringOtherApps: true)
            popover.contentViewController?.view.window?.makeKey()
        }
    }

    /// Eine Datei wird über das Symbol gezogen: Panel im Reiter „Datei“
    /// öffnen, damit sie dort auf eine Sprache gezogen werden kann.
    func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        model.tab = .file
        if !popover.isShown { toggle() }
        return .copy
    }

    /// Die Datei wurde aufs Symbol fallen gelassen: Sie wartet im Reiter
    /// „Datei“ auf die Wahl der Sprache.
    func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let urls = sender.draggingPasteboard.readObjects(
            forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
        guard let files = urls, !files.isEmpty else { return false }
        model.droppedFiles = files
        model.tab = .file
        if !popover.isShown { toggle() }
        return true
    }

    // Ist das Panel offen, hat Diktator den Fokus; ein Diktat geht dann in
    // die Zwischenablage statt ins Panel.
    func popoverDidShow(_ notification: Notification) {
        send("panel", ["visible": true])
        outsideClicks = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) {
            [weak self] _ in
            Task { @MainActor in self?.popover.performClose(nil) }
        }
    }

    func popoverDidClose(_ notification: Notification) {
        send("panel", ["visible": false])
        if let outsideClicks { NSEvent.removeMonitor(outsideClicks) }
        outsideClicks = nil
        model.panelClosed()
    }

    /// Symbol in der Menüleiste.
    func updateButton() {
        guard let button = item?.button else { return }
        let status = model.status
        let (symbol, tint): (String, NSColor?) =
            switch status.state {
            case .loading: ("hourglass", nil)
            case .ready: ("waveform", nil)
            case .recording: ("mic.fill", .systemRed)
            case .processing: ("ellipsis.circle", nil)
            case .polishing: ("sparkles", nil)
            case .sourceLoading: ("arrow.down.circle", nil)
            case .transcribing: ("doc.text", nil)
            case .translating: ("character.book.closed", nil)
            case .problem: ("exclamationmark.triangle.fill", .systemRed)
            }
        var configuration = NSImage.SymbolConfiguration(pointSize: 15, weight: .medium)
        if let tint {
            // Farbige Symbole dürfen keine Vorlage sein, sonst färbt macOS sie
            // wie die Menüleiste ein.
            configuration = configuration.applying(.init(paletteColors: [tint]))
        }
        let symbolImage = NSImage(systemSymbolName: symbol, accessibilityDescription: "Diktator")?
            .withSymbolConfiguration(configuration)
        // Jedes Symbol auf derselben Fläche: Sonst setzt macOS das offene
        // Panel neu an, wenn das Mikrofon (etwas höher) die Wellenform ablöst.
        let canvas = NSSize(width: 20, height: 20)
        let image = NSImage(size: canvas, flipped: false) { rect in
            guard let symbolImage else { return false }
            let size = symbolImage.size
            symbolImage.draw(in: NSRect(
                x: (rect.width - size.width) / 2, y: (rect.height - size.height) / 2,
                width: size.width, height: size.height))
            return true
        }
        image.isTemplate = tint == nil
        button.image = image
    }

    /// Liest Nachrichten von Diktator; endet die Eingabe, endet das Programm.
    func startReading() {
        Thread.detachNewThread { [weak self] in
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            while let line = readLine() {
                let message: Incoming
                do {
                    message = try decoder.decode(Incoming.self, from: Data(line.utf8))
                } catch {
                    // Ins Protokoll von Diktator, ohne die Zeile selbst (sie kann
                    // Wörterbuch-Einträge und Pfade enthalten).
                    // Vom Hauptthread wie alle Befehle, damit sich Zeilen nicht mischen.
                    let message = "Nachricht unlesbar (\(line.count) Zeichen): \(error)"
                    Task { @MainActor in send("log", ["message": message]) }
                    continue
                }
                Task { @MainActor in
                    self?.model.apply(message)
                    self?.updateButton()
                    if let state = self?.model.status.state {
                        self?.indicator.follow(state: state)
                    }
                }
            }
            Task { @MainActor in NSApp.terminate(nil) }
        }
    }
}
