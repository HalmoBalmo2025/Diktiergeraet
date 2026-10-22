# Diktator

[![Prüfung](https://github.com/HalmoBalmo2025/Dictator/actions/workflows/ci.yml/badge.svg)](https://github.com/HalmoBalmo2025/Dictator/actions/workflows/ci.yml)

Lokales Diktieren in jede Mac-App und YouTube-Transkripte – mit
[Cohere Transcribe](https://huggingface.co/CohereLabs/cohere-transcribe-03-2026),
ohne Cloud, ohne Account, ohne Kosten. Ein Rust-Programm mit kleinen
Swift-Hilfsprogrammen für die Oberfläche und macOS-Dienste, das als Symbol
in der Menüleiste läuft.

Alles läuft auf dem Mac: Aufnahmen, Transkripte und Diktate verlassen ihn nicht
(nur YouTube-Videos werden zum Transkribieren heruntergeladen).

## Kurz: App neu bauen und starten

```sh
~/Documents/Projekte/Diktator/scripts/bundle.sh
```

Geht von jedem Ordner im Terminal aus. Baut die App aus dem aktuellen Code, legt sie nach
`~/Applications/Diktator.app`, beendet ein laufendes Diktator und startet die neue Version.
Liegt das Projekt woanders, den Pfad davor anpassen. Was beim ersten Mal dazugehört (Rust,
Homebrew-Pakete, Modell), steht unter [Voraussetzungen](#voraussetzungen).

## Funktionen

**Diktieren in jede App**

| Taste | Wirkung |
|---|---|
| **fn** antippen | Aufnahme auf Deutsch starten |
| **⇧ (Umschalt) + fn** | Aufnahme in der zweiten Sprache starten (Standard Englisch, im Panel unter Einstellungen wählbar) |
| **fn** erneut | Aufnahme beenden, Text an der Schreibmarke einfügen |
| **fn + ⌥** (auch mit ⇧) | Diktieren und vor dem Einfügen korrigieren lassen (Rechtschreibung, Grammatik, Zeichensetzung) – mit Apple Intelligence oder einem lokalen Sprachmodell über llama.cpp (Einstellungen → Überarbeiten). Übernommen werden nur Korrekturen, keine eingefügten oder gestrichenen Wörter; richtig geschriebene Wörter (macOS-Rechtschreibprüfung), Zahlen und Verneinungen („wirksam“/„unwirksam“) bleiben unangetastet. |
| **Esc** | Diktat verwerfen, auch während der Verarbeitung (erkannter Text steht im Verlauf); beim Überarbeiten: abbrechen und den rohen Text einfügen |

- Längere Diktate erscheinen schon während des Sprechens: Ab 5 s wird an einer längeren Pause (Satzende),
  ab 10 s an jeder Sprechpause, spätestens nach 25 s ein Teil abgeschnitten, transkribiert und eingefügt,
  während die Aufnahme weiterläuft.
  Esc verwirft den Rest; schon eingefügter Text bleibt.
  Der Schieber „Text beim Sprechen“ (Einstellungen → Diktat) stellt die 5 s auf 10, 15 oder 20 s oder
  „am Ende“; die übrigen Grenzen wachsen mit (das Doppelte, das Fünffache), ein Teil ist aber nie länger
  als 28 s (Diktator gibt dem Modell höchstens 30 s am Stück). Früh geschnitten erscheint der Text
  schneller, der Schnitt liegt aber öfter mitten im Satz, wo das Modell falsche Satzzeichen setzt; „am
  Ende“ sieht es jeden Satz ganz – Diktate über 30 s teilt Diktator dann an leisen Stellen. Gemessene
  Grenzen stehen in `docs/grenzen.md`.
- Satzzeichen und Großschreibung liefert das Modell selbst. Mitten im Satz passt Diktator
  den Anfang (klein) und das Ende (kein Punkt) an den Text um die Schreibmarke an.
- Je App: In Terminals und Chats (Nachrichten, WhatsApp, Slack, Discord, Telegram, Signal, Teams …)
  fällt der Schlusspunkt weg; in Mail-Programmen (Mail, Outlook, Spark, Superhuman) bleiben es
  vollständige Sätze mit großem Anfang und Punkt.
- Sprachbefehle: „neuer Absatz“ fügt eine Leerzeile ein, „neue Zeile“ einen Zeilenumbruch
  (auch „new paragraph“ / „new line“) – nur am Satzanfang oder nach einem Satzzeichen
  (`. , : ; ! ?`); mitten im Satz („wurde ein neuer Absatz eingefügt“) bleiben die Wörter stehen.
- Während der Aufnahme schwebt an der Schreibmarke ein kleiner Hinweis: roter Punkt (lila beim
  Überarbeiten) mit Pegel, danach Pünktchen, bis der Text eingefügt ist. Verrät eine App ihre
  Schreibmarke nicht (manche Web- und Electron-Apps, Terminals), erscheint er am Mauszeiger.
- Die Zwischenablage bleibt erhalten, soweit sie Text oder ein Bild enthält: Sie wird vor dem Einfügen
  gesichert und danach wiederhergestellt. Formatierter Text kommt als reiner Text zurück; andere
  Inhalte (kopierte Dateien, Tabellen) gehen verloren. Lässt sie sich nicht wiederherstellen, sagt
  die Meldung das.
  Das Diktat ist dabei als „nicht speichern“ markiert, damit Zwischenablage-Verläufe (Raycast, Maccy …) es nicht aufheben.
- Aufgenommen wird immer mit dem eingebauten Mikrofon des Macs, auch wenn macOS gerade AirPods oder das iPhone als Eingang gewählt hat.
- fn + Pfeil, fn + F-Tasten und die Globus-Kürzel lösen nichts aus.
- Im Panel (Reiter „Diktat“) stehen die letzten 10 Diktate; ein Klick kopiert den Text (nur bis zum Beenden gespeichert).
- Ist das Panel beim Start der Aufnahme offen, landet das ganze Diktat in der Zwischenablage (dann mit ⌘V in der gewünschten App einfügen),
  auch wenn das Panel währenddessen schließt; war es geschlossen, wird alles eingefügt.
- Fällt das Mikrofon mitten im Diktat aus, endet es mit dem, was bis dahin aufgenommen war; beim Beenden von Diktator
  kommt erkannter Text eines laufenden Diktats in Verlauf und Zwischenablage.

**YouTube- und Datei-Transkripte**

1. Link im Browser kopieren.
2. Im Panel, Reiter „YouTube“: „YouTube-Video“ übernimmt die Sprache, die YouTube für das Video angibt;
   „Sprache selbst wählen“ nimmt die darunter gewählte (alle 14 Sprachen des Modells, Standard Deutsch).
   Liegt die Maus auf einem der Knöpfe, färbt er sich grün, wenn ein YouTube-Link in der
   Zwischenablage liegt, sonst rot. Geprüft wird nur der Link, nicht, ob das Video abrufbar ist.
3. Das Symbol in der Menüleiste zeigt, dass geladen bzw. transkribiert wird, das Panel den Fortschritt in Prozent; am Ende liegen Titel und Transkript (in Absätzen) in der Zwischenablage.

**Übersetzen:** „Transkribieren und übersetzen“ transkribiert in der Originalsprache und lässt das Transkript danach
vom Sprachmodell übersetzen (dasselbe wie beim Überarbeiten: Apple Intelligence oder das lokale Modell);
Zielsprache darunter wählen, Standard Deutsch. Übersetzt wird Absatz für Absatz; Zeitmarken und
Kapitelüberschriften bekommt das Modell nicht zu sehen, sie stehen danach vor demselben Absatz wie im
Original. Ist das Video schon in der Zielsprache, wird nichts übersetzt. Scheitert das Modell an einem
Absatz, bleibt er im Original stehen (die Meldung sagt, wie viele); „Abbrechen“ kopiert das Original.
Diktate mit fn + ⌥ kommen zwischen zwei Absätzen dran. Gemma 4 12B braucht etwa 3 s pro Absatz, Apple
Intelligence etwa 5 s.

Audio- und Videodateien (z. B. Vorlesungsmitschnitte; geprüft: MP3, M4A, AAC, WAV, AIFF, FLAC, OGG, Opus, WMA, MP4, MOV, M4V, MKV, WebM, AVI – dazu alles, was ffmpeg kennt): Datei aus dem
Finder aufs Menüleisten-Symbol oder auf die große Ablagefläche im Reiter „Datei“ ziehen (sie färbt sich beim
Drüberziehen; aufs Symbol gezogen, öffnet sich das Panel in diesem Reiter) und dort unter „Datei
transkribieren“ die Sprache wählen (alle 14 des Modells, Standard Deutsch) und starten; direkt auf „Datei
transkribieren“ gezogen, geht es sofort los. Alternativ: Datei im Finder auswählen, ⌘C, im Reiter „Datei“
starten; ein mit ⌥⌘C kopierter Pfad geht auch. Mehrere Dateien auf
einmal (gezogen oder kopiert) werden nacheinander transkribiert; was dazukommt, während schon eine läuft,
reiht sich ein. „Abbrechen“ stoppt die laufende und verwirft die wartenden. Der Dateiname wird zur Überschrift. Das fertige Transkript liegt zusätzlich als Textdatei neben der Datei (`Vorlesung.m4a` → `Vorlesung.txt`; gibt es die schon, `Vorlesung (2).txt` – nichts wird überschrieben).

Diktate haben Vorrang: Während ein Video oder eine Datei transkribiert wird, kann weiter diktiert werden.
Fortschritt, „Abbrechen“ und „Letztes Transkript kopieren“ stehen in beiden Reitern.

Der Schalter „Zeitmarken“ (unter „Transkript anpassen“ in beiden Reitern, gilt für beide) setzt vor jeden Absatz, wann er beginnt –
`[12:30]`, ab einer Stunde `[1:02:05]`. Die Zeit ist auf wenige Sekunden genau.

Hat ein YouTube-Video Kapitel, beginnt jedes im Transkript mit seinem Titel als Überschrift
(`## Vertragsschluss`). Die Kapitelgrenze ist nur auf wenige Sekunden genau; die Überschrift rückt
deshalb an den nächstgelegenen Satzanfang (höchstens 30 Sekunden weit), damit sie keinen Satz zerreißt.
Der Schalter „Kapitelüberschriften“ unter „Transkript anpassen“ im Reiter YouTube schaltet das ab
(gilt sofort, gespeichert in `settings.toml`).

Nur einen Teil transkribieren: In beiden Reitern unter „Transkript anpassen“ **von** und **bis** eintragen
(`12:30`, `1:02:05` oder nur Minuten wie `12`), dann die Sprache wählen. Leeres „von“ heißt ab dem
Anfang, leeres „bis“ bis zum Ende. Bei YouTube wird nur dieser Teil heruntergeladen. Der Titel
nennt den Bereich („Vorlesung (12:30–20:00)“), Zeitmarken und Kapitel zählen ab dem Anfang des
Videos bzw. der Datei. Bei Dateien geht ein Bereich nur für eine einzelne Datei; nach dem Start
werden die Felder geleert.

**Wörterbuch und Jura-Regeln**

Normzitate werden bei deutschen Diktaten und Videos in Klausurschreibweise gebracht:

| Gesprochen / vom Modell geschrieben | Eingefügt |
|---|---|
| Paragraf 433 Absatz 2 Satz 1 BGB | § 433 II 1 BGB |
| 812 Absatz 1 Satz 1 erste Alternative BGB | § 812 I 1 Alt. 1 BGB |
| Artikel 2 Absatz 1 in Verbindung mit Artikel 1 Absatz 1 Grundgesetz | Art. 2 I i.V.m. Art. 1 I GG |
| Paragraphen 177 und 179 BGB | §§ 177 und 179 BGB |
| Paragraph 242 St. G. B. | § 242 StGB |

Eine Zahl ohne „Paragraf“ davor wird nur zur Norm, wenn ein Absatz, Satz, Gesetz o. Ä. folgt – „50 Euro“ bleibt.
Im Panel, Reiter „Wörterbuch“: Jura-Regeln an/aus, Schreibweise (`§ 433 II` oder `Abs. 2`), eigene
Ersetzungen (`Extunknichtigkeit` → `Ex-tunc-Nichtigkeit`), Fachbegriffe und weitere Gesetzeskürzel.
Fachbegriffe helfen bei Wörtern, die das Modell knapp verfehlt: Ein erkanntes Wort, das einem Begriff
sehr ähnlich ist (ab 5 Buchstaben, 1 Zeichen Abweichung, ab 8 Buchstaben 2), wird zu ihm –
„Subsomo“ → `Subsumo`; gebeugte Formen wie „Subsumos“ bleiben. Gespeichert wird
in `dictionary.toml`; Änderungen gelten ab dem nächsten Diktat. „Datei öffnen“ zeigt die Datei.

Gesprochene E-Mail-Adressen werden in Diktaten zusammengesetzt (in Transkripten nicht, dort ist „at example.com“ meist eine Webadresse): „peter punkt hase at icloud punkt com“
→ `peter.hase@icloud.com` („ät“, „unterstrich“, „minus“ gehen auch).

**Einstellungen**

Im Panel, Reiter „Einstellungen“ (gespeichert in `~/Library/Application Support/Diktator/settings.toml`):

| Abschnitt | Einstellung | Standard |
|---|---|---|
| `[tasten]` | `diktat`: `"fn"`, `"rechte_wahltaste"`, `"rechte_befehlstaste"`, `"rechte_controltaste"` | `"fn"` |
| `[sprachen]` | `zweite`: Sprache für ⇧ fn – `en`, `fr`, `es`, `it`, `pt`, `nl`, `pl`, `el`, `ar`, `ja`, `zh`, `vi`, `ko` (gilt sofort) | `"en"` |
| `[toene]` | `an`, `lautstaerke` (Prozent) | an, 25 |
| `[modell]` | `datei` im Ordner `models/` (z. B. eine kleinere Stufe) | `…-Q8_0.gguf` |
| `[protokoll]` | `an`: Protokoll schreiben (Schalter im Reiter Info, gilt sofort) | an |
| `[modell]` | `entladen_nach_minuten`: Modell nach Leerlauf entladen (≈ 2,5 GB frei), `0` = nie | 30 |
| `[ueberarbeiten]` | `mit`: `"apple"` (Apple Intelligence) oder `"lokal"` (llama.cpp); `modell`: GGUF-Datei in `models/llm/`, leer = die erste dort; `entladen_nach_minuten`: lokales Modell nach Leerlauf entladen, `0` = nie | `"apple"`, leer, 10 |
| `[transkripte]` | `zeitmarken`: `[12:30]` vor jedem Absatz von YouTube- und Datei-Transkripten | aus |
| `[transkripte]` | `kapitel`: Kapitel eines YouTube-Videos als Überschriften (`## Titel`) im Transkript | an |
| `[diktat]` | `teile_ab_sekunden`: ab wann ein Diktat schon beim Sprechen in Teilen erscheint, 1 bis 20; `0` = erst am Ende (dann endet eine Aufnahme nach 10 Minuten von selbst); gilt ab der nächsten Aufnahme | 5 |
| `[oberflaeche]` | `sprache`: `"de"` oder `"en"` (Schalter „English“ oben im Reiter Info, gilt sofort für Panel, Fehlermeldungen und Mitteilungen; das Protokoll bleibt deutsch) | `"de"` |

Änderungen im Panel gelten sofort – nur Taste und Modelldatei brauchen einen Neustart; dann erscheint oben im Reiter ein oranger Hinweis mit „Jetzt neu starten“. Von Hand in `settings.toml` Geändertes gilt, sobald das Panel geöffnet wird (Taste und Modelldatei wieder erst nach dem Neustart, mit demselben Hinweis). Speichert das Panel, schreibt es `settings.toml` neu; eigene Kommentare darin gehen dabei verloren (wie beim Wörterbuch). Der Schalter „Beim Anmelden starten“ richtet den Autostart ein
(`~/Library/LaunchAgents/local.diktator.plist`).

Die erste Zeile von `settings.toml` und `dictionary.toml` (`# Dateiformat / file format: 2`)
nennt das Format der Datei; sie bleibt stehen. Dateien früherer Versionen liest Diktator weiter:
Ein Wert, den es so nicht mehr gibt (etwa `teile_ab_sekunden` über 20 oder `zweite = "de"`), wird
ersetzt und beim Start gemeldet. Eine Datei, die eine neuere Version geschrieben hat, bleibt
unangetastet; Diktator meldet das und speichert nicht darüber.

**Menüleiste und Panel**

Ein Klick auf das Symbol öffnet mittig darunter ein Panel, ein Klick daneben (auch auf den leeren
Schreibtisch) schließt es. Reiter: Diktat (Tasten, Verlauf; liegt die Maus auf einem Eintrag, steht
der ganze Text daneben),
YouTube, Datei, Wörterbuch, Einstellungen, Info (Sprache Deutsch/Englisch, Version, Modell, Rechte, fn-Taste freigeben, Protokoll an/aus, öffnen, löschen); ganz rechts in
der Reiterleiste beendet ⏻ Diktator. Oben steht der Zustand und was zuletzt passiert ist.

| Symbol | Bedeutung |
|---|---|
| Sanduhr | Modell lädt |
| Wellenform | bereit |
| rotes Mikrofon | Aufnahme läuft (Sprache steht im Panel; das orange Mikrofon daneben ist die Anzeige von macOS) |
| Kreis mit Punkten | Aufnahme wird transkribiert |
| Funkeln | Diktat wird überarbeitet (Esc fügt den rohen Text ein) |
| Pfeil nach unten / Dokument | Video oder Datei wird geladen / transkribiert (Prozent im Panel) |
| Buch | Transkript wird übersetzt (Prozent im Panel) |
| rotes Warndreieck | Problem (Details oben im Panel) |

## Voraussetzungen

- Mac mit Apple Silicon, macOS 14 oder neuer (Überarbeiten und Übersetzen mit Apple Intelligence ab macOS 27; das lokale Modell geht auch davor)
- [Rust](https://rustup.rs) (Edition 2024)
- Xcode-Kommandozeilenwerkzeuge (`xcode-select --install`) für `swiftc` und `codesign`
- Homebrew-Pakete (CMake zum Bauen, yt-dlp, FFmpeg und Deno für YouTube und Dateien):

  ```sh
  brew install cmake yt-dlp ffmpeg deno
  ```

- Das Modell (GGUF, ~2,4 GB, Apache-2.0, kein Hugging-Face-Account nötig):

  ```sh
  mkdir -p ~/Library/Application\ Support/Diktator/models
  curl -L -o ~/Library/Application\ Support/Diktator/models/cohere-transcribe-03-2026-Q8_0.gguf \
    https://huggingface.co/handy-computer/cohere-transcribe-03-2026-gguf/resolve/main/cohere-transcribe-03-2026-Q8_0.gguf
  ```

- Systemeinstellungen → Tastatur → „🌐-Taste drücken für“ auf **„Keine Aktion“**.
- Optional fürs Überarbeiten mit einem lokalen Sprachmodell statt Apple Intelligence:
  llama.cpp und ein Modell (GGUF) im Ordner `models/llm`, z. B. Gemma 4 12B (6,7 GB,
  Apache-2.0, kein Account nötig). Diktator startet `llama-server` selbst, wenn es
  gebraucht wird, und beendet es nach der eingestellten Pause wieder.

  ```sh
  brew install llama.cpp
  mkdir -p ~/Library/Application\ Support/Diktator/models/llm
  curl -L -C - -o ~/Library/Application\ Support/Diktator/models/llm/gemma-4-12B-it-qat-UD-Q4_K_XL.gguf \
    https://huggingface.co/unsloth/gemma-4-12B-it-qat-GGUF/resolve/main/gemma-4-12B-it-qat-UD-Q4_K_XL.gguf
  ```

## Bauen und starten

```sh
cargo run --release          # aus dem Terminal (Rechte gelten dann fürs Terminal)
./scripts/bundle.sh          # baut ~/Applications/Diktator.app und startet sie (neu)
```

Beim ersten Start fragt macOS nach den Rechten **Mikrofon**, **Bedienungshilfen**
(⌘V senden, Esc abfangen, Schreibmarke lesen) und **Eingabeüberwachung** (fn mitlesen).
Fehlte beim Start eines der beiden letzten, Diktator nach dem Erteilen neu starten.
Autostart: Schalter „Beim Anmelden starten“ im Panel (Reiter „Einstellungen“).

Mit der lokalen Signatur fragt macOS die Rechte nach jedem Neubau erneut ab.
Dauerhaft bleiben sie mit einem eigenen Zertifikat – die Anleitung steht oben in
`scripts/bundle.sh`.

## Fehlersuche

- Stürzt Diktator ab, startet es nach einer Sekunde von selbst neu (ein kleiner Wächter-Prozess
  startet die eigentliche App als Kindprozess; in der Aktivitätsanzeige stehen deshalb zwei
  „Diktator“). Nach drei Abstürzen in zwei Minuten bleibt es aus und meldet das. Kann die App
  gar nicht starten, versucht der Wächter es nicht erneut, sondern meldet es; der Grund steht
  im Protokoll. Auch eine Panik hinterlässt dort Ort und Thread (ohne die Meldung selbst). Wird
  die App von außen beendet (Aktivitätsanzeige, `kill`), gilt das als „Beenden“, nicht als Absturz.

- Protokoll: `~/Library/Logs/Diktator.log` (Datum und Ortszeit), z. B. `tail -30 ~/Library/Logs/Diktator.log`
- Wird keine Sprache erkannt (oder verwirft Diktator, was das Modell erfand), liegt die
  Aufnahme bei eingeschaltetem Protokoll zum Nachhören unter
  `~/Library/Application Support/Diktator/letzte-leere-aufnahme.wav`.
- Oben im Panel steht das letzte Ergebnis oder die letzte Fehlermeldung.
- Das Protokoll nennt bei jedem Einfügen die App und ihre Regel (`App com.apple.mail: vollständige Sätze`)
  und ob die Schreibmarke lesbar war – ohne Text aus dem Diktat oder um die Schreibmarke.
- Wörterbuch: `~/Library/Application Support/Diktator/dictionary.toml`; Tippfehler darin meldet eine Mitteilung.
- Lokales Sprachmodell: Ausgabe von `llama-server` in `~/Library/Logs/Diktator-llama.log`.

## Aufbau

| Modul | Aufgabe |
|---|---|
| `src/main.rs` | Startet alles, nimmt Befehle aus dem Panel an, baut in fester Reihenfolge ab |
| `src/watchdog.rs` | Wächter: startet die App als Kindprozess und nach einem Absturz neu |
| `src/app/ui.rs`, `helpers/ui/` | Menüleisten-Symbol, Panel und Hinweis an der Schreibmarke (Swift-Hilfsprogramm mit SwiftUI, JSON-Zeilen über stdin/stdout; eine Datei je Reiter bzw. Thema) |
| `src/app/controller/` | Zustandsautomat: Diktat (`dictation.rs`), YouTube und Dateien (`transcript.rs`) |
| `src/app/status.rs`, `src/app/history.rs` | Nachrichten an die Oberfläche; Verlauf der letzten Diktate |
| `src/app/worker.rs` | Modell-Thread mit Warteschlange (Diktat vor YouTube) |
| `src/engine.rs` | Cohere Transcribe über [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) (Metal) |
| `src/hotkey.rs` | fn / ⇧ fn / Esc über einen `CGEventTap` |
| `src/audio/` | Aufnahme (cpal), 16 kHz, Stille-Erkennung, Abschnitte ≤ 30 s, WAV, Dateien dekodieren (`decode.rs`), Bereich „von–bis“ (`range.rs`) |
| `src/polish/`, `helpers/llm.swift`, `helpers/spell.swift` | Überarbeiten-Thread (`worker.rs`): Apple Intelligence (`apple.rs`) oder lokal über `llama-server` (`llama.rs`, `local.rs`), Sicherheitsnetz (`guard.rs`) mit macOS-Rechtschreibprüfung (`spelling.rs`); Übersetzen langer Transkripte Absatz für Absatz (`translate.rs`) |
| `src/process.rs` | Kurze Kindprozesse: Eingabe und beide Ausgaben auf eigenen Threads, Zeitgrenze und Abbruch, Einsammeln |
| `src/helper.rs` | Swift-Hilfsprogramme finden und über `process.rs` aufrufen |
| `src/context.rs`, `helpers/context.swift` | App vorn und Text um die Schreibmarke lesen (Swift-Hilfsprogramm, Bedienungshilfen) |
| `src/paste.rs` | Einfügen per ⌘V, Zwischenablage sichern und wiederherstellen |
| `src/app/source.rs` | Quelle eines langen Transkripts: YouTube-Video oder Datei |
| `src/youtube.rs` | Link prüfen, yt-dlp → ffmpeg ohne Zwischendatei, Abbruch |
| `src/feedback.rs` | Klänge, Mitteilungen, Protokoll |
| `src/settings.rs` | `settings.toml`: Taste, zweite Sprache, Klänge, Modelle, Protokoll, Überarbeiten, Zeitmarken |
| `src/launch.rs` | Autostart (`LaunchAgent`) |
| `src/textfile.rs` | Wörterbuch und Einstellungen anlegen und sicher ersetzen, das Wörterbuch im Editor öffnen |
| `src/paths.rs`, `src/language.rs` | Ablageorte und Werkzeuge im `PATH`; die Sprachen des Modells |
| `src/text/` | Nachbearbeitung in fester Reihenfolge (`mod.rs`): Geräusch-Vermerke und erfundene Sätze (`phantom.rs`), Sprachbefehle (`commands.rs`), E-Mail-Adressen (`email.rs`), Wörterbuch (`dictionary.rs`), Fachbegriffe (`terms.rs`), Jura-Regeln (`legal/`, Gesetze in `statutes.rs`), Zahlen (`figures.rs`), Absätze und Zeitmarken; Anpassen an Schreibmarke und App (`fit.rs`) |

Die Entstehung, Entscheidungen und nächsten Schritte stehen in [`PLAN.md`](PLAN.md).

## Entwicklung

```sh
cargo fmt --check
cargo clippy --all-targets --locked   # muss ohne Befund durchlaufen (strenge Regeln in Cargo.toml)
cargo test --locked                   # auch die Abläufe der Steuerung, mit nachgebautem Mac
cargo doc --no-deps --document-private-items --locked   # ohne Warnungen
# Dieselben Prüfungen laufen auf GitHub bei jedem Push auf main (.github/workflows/ci.yml).
# Panel und Hinweis an der Schreibmarke als PNG (hell und dunkel):
xcrun swiftc -parse-as-library -swift-version 5 -o /tmp/diktator-ui helpers/ui/*.swift && /tmp/diktator-ui --snapshot /tmp
```

Kein `unsafe`, keine Panics, Fehler als Enums – die Richtlinien stehen im allgemeinen Teil der
`Cargo.toml` (für LLMs verweist `CLAUDE.md` darauf). Die Rust-Version ist in `rust-toolchain.toml`
festgelegt, rustup installiert sie beim ersten Bauen; sie und die Dependencies werden nur in einem
bewussten Update-Schritt angehoben (Richtlinie 10, mit `cargo audit`).

## Bausteine

- [Cohere Transcribe](https://huggingface.co/CohereLabs/cohere-transcribe-03-2026) (Apache-2.0), GGUF von [handy-computer](https://huggingface.co/handy-computer/cohere-transcribe-03-2026-gguf)
- [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) (MIT)
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) und [FFmpeg](https://ffmpeg.org)
