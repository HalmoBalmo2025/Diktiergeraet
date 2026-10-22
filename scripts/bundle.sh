#!/bin/sh
# Baut Diktator.app aus dem Release-Binary, legt sie nach ~/Applications und
# startet sie neu (eine laufende Diktator-App wird dafür beendet, auch mitten
# in einem Transkript).
#
#   ./scripts/bundle.sh
#
# Signatur: Ohne weiteres wird lokal ("ad hoc") signiert. Dann sieht macOS
# jede neu gebaute App als neues Programm und fragt die Rechte erneut ab.
# Dauerhaft bleiben die Rechte mit einem eigenen Zertifikat:
#   Schlüsselbundverwaltung → Schlüsselbundverwaltung (Menü) → Zertifikatsassistent
#   → Zertifikat erstellen … → Name „Diktator Dev“, Identitätstyp
#   „Selbstsigniertes Stammzertifikat“, Zertifikattyp „Codesignatur“.
# Liegt ein Zertifikat „Diktator Dev“ im Schlüsselbund, wird es automatisch
# verwendet; ein anderes wählt man mit  DIKTATOR_SIGN_ID="Name" ./scripts/bundle.sh
set -eu
cd "$(dirname "$0")/.."

cargo build --release --locked

# Version aus der Cargo.toml (erste `version`-Zeile, die des Pakets).
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)

APP="target/Diktator.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/diktator "$APP/Contents/MacOS/Diktator"
# Ältestes macOS, auf dem die App läuft (wie LSMinimumSystemVersion unten);
# ohne festes Ziel gälte das macOS, auf dem gebaut wird.
TARGET="arm64-apple-macos14.0"
# Swift-Hilfsprogramm: liest den Text um die Schreibmarke (Bedienungshilfen).
xcrun swiftc -O -target "$TARGET" -o "$APP/Contents/MacOS/diktator-context" helpers/context.swift
# Swift-Hilfsprogramm: Menüleisten-Symbol und Panel (SwiftUI).
xcrun swiftc -O -target "$TARGET" -parse-as-library -swift-version 5 -o "$APP/Contents/MacOS/diktator-ui" helpers/ui/*.swift
# Swift-Hilfsprogramm: überarbeitet Text mit Apple Intelligence (Foundation
# Models). Braucht macOS 27; auf älteren scheitert nur das Überarbeiten mit
# Apple Intelligence (Diktator fügt dann roh ein).
xcrun swiftc -O -target arm64-apple-macos27.0 -parse-as-library -o "$APP/Contents/MacOS/diktator-llm" helpers/llm.swift
# Swift-Hilfsprogramm: Rechtschreibprüfung von macOS (Sicherheitsnetz fürs Überarbeiten).
xcrun swiftc -O -target "$TARGET" -o "$APP/Contents/MacOS/diktator-spell" helpers/spell.swift

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Diktator</string>
    <key>CFBundleDisplayName</key>
    <string>Diktator</string>
    <key>CFBundleIdentifier</key>
    <string>local.diktator</string>
    <key>CFBundleExecutable</key>
    <string>Diktator</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>Diktator nimmt deine Sprache auf und wandelt sie lokal in Text um.</string>
</dict>
</plist>
PLIST

if [ -z "${DIKTATOR_SIGN_ID:-}" ] && security find-identity -p codesigning | grep -q '"Diktator Dev"'; then
    DIKTATOR_SIGN_ID="Diktator Dev"
fi
SIGN_ID="${DIKTATOR_SIGN_ID:--}"
codesign --force --sign "$SIGN_ID" --identifier local.diktator.context "$APP/Contents/MacOS/diktator-context"
codesign --force --sign "$SIGN_ID" --identifier local.diktator.ui "$APP/Contents/MacOS/diktator-ui"
codesign --force --sign "$SIGN_ID" --identifier local.diktator.llm "$APP/Contents/MacOS/diktator-llm"
codesign --force --sign "$SIGN_ID" --identifier local.diktator.spell "$APP/Contents/MacOS/diktator-spell"
codesign --force --sign "$SIGN_ID" --identifier local.diktator "$APP"

# Laufendes Diktator beenden: Nur der Wächter bekommt das Signal (seine
# Befehlszeile endet auf „Diktator“, die des Kindes auf „--kind“); das Kind
# merkt es an der Pipe und räumt selbst auf (Hilfsprogramme, llama-server).
# Hängt nach 10 s noch etwas aus dem Bundle, wird es hart beendet.
if pkill -f 'Diktator\.app/Contents/MacOS/Diktator$'; then
    waited=0
    while pgrep -f 'Diktator\.app/Contents/MacOS/' >/dev/null; do
        if [ "$waited" -ge 50 ]; then
            pkill -KILL -f 'Diktator\.app/Contents/MacOS/' || true
            break
        fi
        waited=$((waited + 1))
        sleep 0.2
    done
fi

mkdir -p "$HOME/Applications"
rm -rf "$HOME/Applications/Diktator.app"
cp -R "$APP" "$HOME/Applications/"
open "$HOME/Applications/Diktator.app"
echo "Fertig und gestartet: ~/Applications/Diktator.app (Signatur: $SIGN_ID)"
