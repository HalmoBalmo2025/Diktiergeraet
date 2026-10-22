// Hinweis an der Schreibmarke: schwebender Punkt mit Pegel während der Aufnahme.

import AppKit
import SwiftUI

/// Kleines schwebendes Fenster an der Schreibmarke: roter Punkt mit Pegel
/// während der Aufnahme, Pünktchen, während der Text entsteht. Es nimmt
/// keinen Fokus und keine Klicks, damit das Einfügen die richtige App trifft.
@MainActor
final class Indicator {
    let model: Model
    let panel: NSPanel
    /// Gerade zu sehen?
    var visible = false

    init(model: Model) {
        self.model = model
        panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 58, height: 24),
            styleMask: [.nonactivatingPanel, .borderless], backing: .buffered, defer: true)
        panel.isFloatingPanel = true
        panel.level = .statusBar
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = true
        panel.ignoresMouseEvents = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        panel.contentView = NSHostingView(rootView: IndicatorView(model: model))
    }

    /// Zeigt den Hinweis bei einer Aufnahme an der Schreibmarke, lässt ihn
    /// stehen, während der Text entsteht, und blendet ihn danach aus.
    func follow(state: AppState) {
        switch state {
        case .recording where !visible:
            model.level = 0
            place()
            panel.orderFrontRegardless()
            visible = true
        case .recording, .processing, .polishing:
            break
        case .loading, .ready, .sourceLoading, .transcribing, .translating, .problem:
            if visible {
                panel.orderOut(nil)
                visible = false
            }
        }
    }

    /// Rechts unter die Schreibmarke; verrät die App sie nicht, an den
    /// Mauszeiger. Immer ganz auf dem Bildschirm.
    func place() {
        let anchor = caretPoint() ?? NSEvent.mouseLocation
        let size = panel.frame.size
        var origin = NSPoint(x: anchor.x + 6, y: anchor.y - size.height - 6)
        let screen = NSScreen.screens.first { NSMouseInRect(anchor, $0.frame, false) } ?? NSScreen.main
        if let frame = screen?.visibleFrame {
            origin.x = min(max(origin.x, frame.minX), frame.maxX - size.width)
            origin.y = min(max(origin.y, frame.minY), frame.maxY - size.height)
        }
        panel.setFrameOrigin(origin)
    }

    /// Untere linke Ecke der Schreibmarke in Bildschirmkoordinaten (AppKit:
    /// Ursprung unten links), über die Bedienungshilfen-API der App vorn.
    func caretPoint() -> NSPoint? {
        guard AXIsProcessTrusted(), let front = NSWorkspace.shared.frontmostApplication else { return nil }
        let app = AXUIElementCreateApplication(front.processIdentifier)
        AXUIElementSetMessagingTimeout(app, 0.2)
        AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        var focusedRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &focusedRef) == .success,
              let focusedRef, CFGetTypeID(focusedRef) == AXUIElementGetTypeID()
        else { return nil }
        let focused = focusedRef as! AXUIElement
        var rangeRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(focused, kAXSelectedTextRangeAttribute as CFString, &rangeRef) == .success,
              let rangeRef
        else { return nil }
        var boundsRef: CFTypeRef?
        guard AXUIElementCopyParameterizedAttributeValue(
                focused, kAXBoundsForRangeParameterizedAttribute as CFString, rangeRef, &boundsRef) == .success,
              let boundsRef, CFGetTypeID(boundsRef) == AXValueGetTypeID()
        else { return nil }
        var rect = CGRect.zero
        guard AXValueGetValue(boundsRef as! AXValue, .cgRect, &rect), rect != .zero else { return nil }
        // Bedienungshilfen zählen von oben links des Hauptbildschirms.
        let height = NSScreen.screens.first?.frame.height ?? 0
        return NSPoint(x: rect.minX, y: height - rect.maxY)
    }
}

/// Inhalt des Hinweises: Punkt und fünf Balken, die mit dem Pegel wachsen.
struct IndicatorView: View {
    @ObservedObject var model: Model

    var body: some View {
        HStack(spacing: 5) {
            if model.status.state == .recording {
                Circle()
                    .fill(model.status.mode == .polish ? Color.purple : Color.red)
                    .frame(width: 8, height: 8)
                HStack(alignment: .center, spacing: 2) {
                    ForEach(0..<5, id: \.self) { bar in
                        Capsule()
                            .fill(Color.white.opacity(model.level > bar * 2 ? 0.95 : 0.3))
                            .frame(width: 3, height: barHeight(bar))
                    }
                }
                .animation(.easeOut(duration: 0.1), value: model.level)
            } else {
                Image(systemName: model.status.state == .polishing ? "sparkles" : "ellipsis")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.white)
            }
        }
        .frame(width: 58, height: 24)
        .background(Capsule().fill(Color.black.opacity(0.78)))
    }

    /// Mittlere Balken sind höher; mit dem Pegel wachsen alle.
    func barHeight(_ bar: Int) -> CGFloat {
        let shape: [CGFloat] = [0.55, 0.8, 1, 0.8, 0.55]
        let loudness = 0.35 + 0.65 * CGFloat(min(model.level, 10)) / 10
        return max(3, 14 * shape[bar] * loudness)
    }
}
