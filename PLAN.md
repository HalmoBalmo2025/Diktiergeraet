# Diktator – Plan

Begonnen am 12.10.2026, zuletzt aktualisiert am 16.10.2026. Lokales Diktieren in jede Mac-App und YouTube-Transkripte, beides mit
Cohere Transcribe, ohne Cloud, ohne Account, ohne Kosten. Erst der Plan, dann Code.

Zielrechner: MacBook Pro M1 Pro, 16 GB, macOS 27.0.1 (ein neuer Mac ist geplant).

## 1. Entscheidungen

| Thema | Entscheidung | Grund |
|---|---|---|
| Modell | Cohere Transcribe 03-2026, GGUF **Q8_0** (2,41 GB) | Auf dem M1 Pro bei Deutsch klar vor Parakeet (kurz 6,9 % vs. 9,8 % WER, lang 17,5 % vs. 34,1 %). Q5_K_M (1,77 GB) als Ausweichstufe, falls der Speicher mit LLM eng wird. |
| Modellquelle | `huggingface.co/handy-computer/cohere-transcribe-03-2026-gguf`, direkter Download | Apache-2.0, nicht zugangsbeschränkt, kein HF-Account. |
| Laufzeit | Crate `transcribe-cpp` 0.4 (transcribe.cpp, Metal) | Rust-Bindings, kein Python. |
| Satzzeichen / Großschreibung | macht das Modell selbst (Standard an) | Kein Zusatzschritt nötig. |
| Sprache | wird pro Aufnahme gewählt, keine Auto-Erkennung | Cohere hat keine Spracherkennung. |
| Programmform | **Rust-Kern + kleines Swift-Hilfsprogramm für die Oberfläche**: Menüleisten-Symbol mit Panel (`NSPopover` + SwiftUI, `diktator-ui`), kein Dock-Symbol. Bis 13.10.2026 `tray-icon` + `tao` mit einfachem Menü. | Kein Webview, schneller Start. Das Panel (Reiter, Karten, Schalter, mittig unter dem Symbol) geht mit der nativen Menüliste nicht; SwiftUI gibt es nur in Swift, Rust bleibt ohne `unsafe`. |
| YouTube | Link aus der Zwischenablage, Ergebnis in die Zwischenablage; `yt-dlp` + `ffmpeg` (Homebrew) | Einfach kopieren und einfügen, keine Datei. |
| Zwischenablage beim Diktieren | nach dem Einfügen alten Inhalt wiederherstellen | Wunsch. |
| Reihenfolge | Diktieren + YouTube → Verbesserungen → LLM-Nachbearbeitung | Wunsch. |

## 2. Bedienung

### Tasten

| Aktion | Taste |
|---|---|
| Aufnahme **Deutsch** starten | **fn** antippen |
| Aufnahme **Englisch** starten | **fn + ⇧** (⇧ gedrückt halten, fn antippen) |
| Aufnahme beenden und einfügen | fn erneut antippen (mit oder ohne ⇧; die Sprache steht seit dem Start fest) |
| Aufnahme abbrechen | **Esc** (wird nur während der Aufnahme abgefangen) |

**„Antippen“ heißt:** fn gedrückt und wieder losgelassen, ohne dass dazwischen eine andere Taste kam, und kürzer als 500 ms. fn+Pfeil (Pos1/Ende), fn+F-Tasten und die Globus-Kürzel bleiben dadurch unberührt. ⇧ ist eine Sondertaste und zählt nicht als „andere Taste“.

**macOS-Einstellung:** Systemeinstellungen → Tastatur → „🌐-Taste drücken für“ auf **„Keine Aktion“**, sonst öffnet fn zusätzlich Emoji, Diktat oder Eingabequelle.

### Menüleiste

Titel: `◦` bereit · `● DE` / `● EN` nimmt auf · `…` verarbeitet · `▶ YT 34 %` YouTube läuft · `⚠` Fehler

```
YouTube aus Zwischenablage (Sprache automatisch)
YouTube aus Zwischenablage – Deutsch
YouTube aus Zwischenablage – Englisch
YouTube abbrechen                      (nur sichtbar, solange eins läuft)
─────────────
Letztes YouTube-Transkript erneut kopieren
─────────────
Beenden
```

Töne über `afplay` mit Systemklängen (Start, Stopp, fertig, Fehler). Mitteilungen über `osascript -e 'display notification …'`. Kein Fenster, das der Ziel-App den Fokus stehlen könnte.

## 3. Ablauf Diktieren

1. fn-Tipp → Mikrofon-Stream öffnen (cpal), Samples sammeln. Der Stream läuft nur während der Aufnahme, der orange Mikrofonpunkt ist also nur dann zu sehen.
2. fn-Tipp → Stream schließen, auf 16 kHz mono f32 umrechnen (rubato).
3. Stille vorne und hinten abschneiden (Energieschwelle). Ist gar keine Sprache drin: nichts einfügen, Ton „nichts erkannt“. Grund: Cohere halluziniert bei Stille.
4. Länger als 30 s → an leisen Stellen in Abschnitte ≤ 30 s teilen (transcribe.cpp nimmt höchstens ~400 s pro Aufruf, Cohere ist auf ~35 s trainiert).
5. Transkribieren im Worker-Thread, der das Modell dauerhaft hält.
6. Einfügen: Zwischenablage sichern (Text/Bild via arboard) → Transkript hinein → ⌘V als Tastencode (Taste V, layoutunabhängig) → ~150 ms warten → alten Inhalt zurück.

Eingefügt wird in die App, die beim Ende der Verarbeitung vorne ist (wie bei Handy). Erste Test-Apps: **Notizen, Mail, Claude**.

## 4. Ablauf YouTube

1. Link im Browser kopieren (⌘L, ⌘C oder „Link kopieren“), dann im Menü „YouTube aus Zwischenablage …“ wählen.
2. Prüfen, ob die Zwischenablage einen YouTube-Link enthält (youtube.com/watch, youtu.be, /shorts, /live). Sonst Ton + Mitteilung „Kein YouTube-Link in der Zwischenablage“.
3. `yt-dlp --print` holt Titel, Dauer und `language`. Bei „automatisch“: `de` → Deutsch, sonst **Englisch** (75 % deiner Videos).
4. `yt-dlp` lädt nur die Tonspur in ein temporäres Verzeichnis → `ffmpeg` wandelt in 16 kHz mono f32 (über stdout, keine WAV-Datei).
5. Abschnitte ≤ 30 s an leisen Stellen schneiden, nacheinander transkribieren; Fortschritt im Menüleisten-Titel, Abbrechen im Menü.
6. Fertig: Text (Absatz bei längerer Sprechpause) **in die Zwischenablage**, Ton + Mitteilung „Transkript kopiert: <Titel> (12.430 Wörter)“. Hier wird die alte Zwischenablage absichtlich *nicht* wiederhergestellt. Das Transkript bleibt im Speicher für „erneut kopieren“.
7. Temporäre Dateien werden entfernt (nur App-eigene Temp-Dateien, nichts in deinen Ordnern).

Grobe Dauer auf dem M1 Pro: ~2–3 min pro Stunde Video. Ein Modell, eine Warteschlange: Ein Diktat während YouTube wartet höchstens bis zum Ende des laufenden Abschnitts (Diktat hat Vorrang).

## 5. Aufbau

Stand 14.10.2026 (der ursprüngliche Entwurf mit `tao`/`tray-icon` ist überholt):

```
Hauptthread: liest Befehle aus dem Panel (diktator-ui, Swift), wendet Einstellungen an
      ▲ Zustand, Meldungen, Pegel (JSON-Zeilen)        │ Befehle
      │                                                 ▼
Steuerung (Zustandsautomat; das laufende Diktat gehört zum Zustand:
           Bereit → Aufnahme → Verarbeitung → [Überarbeiten] → Bereit;
           langes Transkript daneben: Laden → Transkribieren)
  ▲ Tasten            ▲ Takte (Teile, Pegel)     │ Teile / Aufträge          │ Diktat zum Überarbeiten
  │                   │                          ▼                           ▼
Tasten-Thread      Taktgeber           Modell-Thread (Cohere, Warteschlange,   Überarbeiten-Thread
(CGEventTap)                           Diktat vor YouTube, Entladen)         (Apple Intelligence oder
                                                                             llama-server, Rechtschreib-
Aufnahme (cpal, eingebautes Mikrofon) ── Teile ──► Steuerung                 prüfung, Sicherheitsnetz)
Einfügen über die Schnittstelle `Platform` (Mikrofon, Zwischenablage, ⌘V, Schreibmarke, Anzeige) –
in Tests durch einen nachgebauten Mac ersetzt.
```

Die Module und ihre Aufgaben stehen im README (Abschnitt „Aufbau“). Swift-Hilfsprogramme (`helpers/`):
`diktator-ui` (Menüleiste, Panel, Hinweis an der Schreibmarke), `diktator-context` (App vorn und Text um
die Schreibmarke), `diktator-llm` (Apple Intelligence), `diktator-spell` (Rechtschreibprüfung).

Code nach den Richtlinien in der Cargo.toml: kein `unsafe`, kein globaler veränderlicher Zustand, jeder
Thread hat einen Besitzer, Fehler als Enums je Modul, keine Panics. Reine Logik und die Abläufe der
Steuerung haben Unit-Tests.

**Ablage:** `~/Library/Application Support/Diktator/`: `models/` (Cohere), `models/llm/` (lokale
Sprachmodelle), `settings.toml`, `dictionary.toml`, `llama-server.pid`, `letzte-leere-aufnahme.wav`.
Protokolle in `~/Library/Logs/` (`Diktator.log`, `Diktator-llama.log`).

## 6. Einrichtung auf dem Mac (einmalig)

```sh
brew install cmake yt-dlp ffmpeg
mkdir -p ~/Library/Application\ Support/Diktator/models
curl -L -o ~/Library/Application\ Support/Diktator/models/cohere-transcribe-03-2026-Q8_0.gguf \
  https://huggingface.co/handy-computer/cohere-transcribe-03-2026-gguf/resolve/main/cohere-transcribe-03-2026-Q8_0.gguf
```

**App-Paket:** Ein kleines Skript `scripts/bundle.sh` baut aus dem Release-Binary eine `Diktator.app` mit `Info.plist` (`LSUIElement` = kein Dock-Symbol, `NSMicrophoneUsageDescription` = Pflicht für den Mikrofonzugriff) und signiert sie lokal (`codesign -s -`). Kein `dx`, kein Fremdwerkzeug.

**Berechtigungen** (Systemeinstellungen → Datenschutz & Sicherheit): Mikrofon, Bedienungshilfen (⌘V senden, Esc abfangen), Eingabeüberwachung (fn mitlesen). Die Rechte hängen am Programm: Bei `cargo run` gelten sie für Terminal/Editor, im Alltag für `Diktator.app`. Nach einem Neubau mit lokaler Signatur kann macOS die Rechte erneut abfragen; das prüfen wir in Phase 0.

Hinweis zur Arbeitsteilung: Seit dem 13.10.2026 läuft Claude direkt auf dem Mac und kann selbst bauen, testen und die Prüfprogramme ausführen. Was Tasten, Menüklicks oder Rechte-Dialoge braucht, testest du.

## 7. Phasen

### Phase 0 – Machbarkeit (kleine Prüfprogramme, ~1 Abend)
- [x] `transcribe-cpp` baut auf dem M1 Pro mit Metal; je eine deutsche und englische Testaufnahme werden mit `language` = `de`/`en` korrekt transkribiert; Ladezeit und Tempo messen.
- [x] fn-Antippen, fn+⇧ und Esc werden über CGEventTap erkannt; fn+Pfeil löst nichts aus.
- [x] Einfügen per Zwischenablage + ⌘V klappt in Notizen, Mail und Claude; alte Zwischenablage kommt zurück.
- [x] Menüleisten-Symbol mit `tao` + `tray-icon`: kein Dock-Symbol, Titel lässt sich ändern, Menü-Klicks kommen an.

**So wird geprüft** (im Projektordner, Ergebnisse an Claude zurückmelden):

```sh
cargo clippy --all-targets                                   # muss ohne Befund durchlaufen
cargo test                                                   # Unit-Tests der reinen Logik
cargo run --release --example transcribe -- mic de 8         # 8 s Deutsch sprechen
cargo run --release --example transcribe -- mic en 8         # 8 s Englisch sprechen
cargo run --release --example hotkey                         # fn, fn+⇧, Esc, fn+Pfeil probieren
cargo run --release --example paste                          # Countdown, in Notizen/Mail/Claude klicken
cargo run --release --example tray                           # Menüleiste beobachten
```

Beim ersten Start fragt macOS nach Mikrofon, Bedienungshilfen und Eingabeüberwachung – die Rechte gelten dann für das Terminal, aus dem `cargo` läuft.

Abnahme: alle Punkte grün. Sonst Plan an der Stelle anpassen, bevor Phase 1 beginnt.

### Phase 1 – Diktieren
- [x] `scripts/bundle.sh` baut `Diktator.app`; Rechte bleiben nach einem Neubau erhalten – mit eigenem Zertifikat „Diktator Dev“ (geprüft am 13.10.2026 mit geändertem Code)
- [x] Zustandsautomat, Aufnahme, Trimmen, Abschnitte, Worker, Einfügen, Menüleisten-Titel, Töne
- [x] Fehlerfälle: Modell fehlt (Mitteilung mit Hinweis auf den curl-Befehl), Rechte fehlen (welches Recht), Mikrofon weg, Aufnahme leer

Zusätzlich umgesetzt: Protokoll `~/Library/Logs/Diktator.log`, Menüzeile „Zuletzt: …“, Sprach-Erkennung am Grundrauschen statt am Spitzenpegel (leises Mikrofon), Englisch-Erkennung, wenn ⇧ irgendwann während fn gedrückt war.

Abnahme: eine Woche Alltag ohne Absturz, Text landet zuverlässig an der Schreibmarke.

Beobachtungsliste (im Alltag darauf achten): erfundene Sätze in Pausen („Herr Präsident!“ u. Ä.) – kommen sie trotz Rand-Trimmen und Filter noch vor, Phase 3 Punkt 7 (Silero-VAD) angehen. Stand 13.10.2026: „Herr Präsident!“ nicht mehr gesehen, dafür zweimal „… Musik …“ als letzter Teil eines Diktats (1,8 s, fast nur der Tastenklick). Jetzt fällt ein Teil weg, der nur aus einem Geräusch-Vermerk besteht (Musik, Applaus, Lachen …); Vermerke in Klammern oder Sternchen fallen auch mitten im Text weg (`text/phantom.rs`, `drop_noise`).

### Phase 2 – YouTube
- [x] Menüpunkte, Link-Erkennung, Metadaten, Sprachwahl, Download, Abschnitte, Fortschritt, Abbrechen, Zwischenablage, Mitteilung, „erneut kopieren“
- [x] Fehlerfälle: yt-dlp/ffmpeg fehlen, kein/ungültiger Link, Video nicht verfügbar, Abbruch räumt Temp-Dateien auf

Absätze alle ~600 Zeichen nach einem Satzende; Titel steht in der ersten Zeile.

Getestet am 13.10.2026 mit `examples/youtube.rs` (gleicher Ablauf wie der Modell-Thread, ohne Menü):

| Video | Länge | Sprache (automatisch) | Laden | Transkribieren | Wörter |
|---|---|---|---|---|---|
| BGB AT Crashkurs | 75 min | `de` → DE | 12 s | 3:16 min (23× Echtzeit) | 12.343 |
| MIT 9.00SC Psychology, Lec 1 | 50 min | `en-US` → EN | 9 s | 1:54 min (26× Echtzeit) | 8.277 |

Spitzenspeicher 3,1 GB. Text sauber, Absätze stimmen. Über das Menü in der App von Andreas bestätigt (13.10.2026).

Abnahme: ein deutsches und ein englisches Video von ≥ 30 min sauber transkribiert.

### Phase 3 – Verbesserungen (Reihenfolge nach Nutzen, wird nach Phase 2 neu sortiert)
1. [x] Wörterbuch / Wortersetzungen (`dictionary.toml`), inkl. **Jura-Regeln** ohne LLM – umgesetzt am 13.10.2026, siehe unten.
2. [x] ~~`RunOptions.vocabulary` / `prompt`~~ – geht nicht: Cohere meldet in transcribe.cpp 0.4 `Vocabulary`, `ContextPrompt` und `TranscriptPrefix` als nicht unterstützt. Fachbegriffe also übers Wörterbuch.
3. [x] ~~Füllwörter entfernen~~ – nicht nötig: In 20.000 Wörtern beider Testvideos kein „äh“/„ähm“/„uh“/„um“; Cohere lässt sie selbst weg.
4. [x] Verlauf der letzten Diktate im Menü (erneut kopieren) – umgesetzt am 13.10.2026: Untermenü „Verlauf“ mit den letzten 10 Diktaten (Anfang des Textes), Klick legt den ganzen Text in die Zwischenablage. Nur im Speicher, nach einem Neustart leer. Schlägt das Einfügen fehl, verweist die Meldung auf den Verlauf.
5. [x] `settings.toml` (Tasten, Modellstufe, Töne) + Menüpunkt „Einstellungen öffnen“ (öffnet die Datei im Editor); Autostart beim Login – umgesetzt am 13.10.2026: Taste fn oder rechte ⌥/⌘/⌃, Klänge an/aus mit Lautstärke, Modelldatei, Entladezeit. Gelesen beim Start; neuer Menüpunkt „Diktator neu starten“ übernimmt Änderungen. Fehlerhafte Datei → Mitteilung, Standardwerte. Autostart als Häkchen im Menü (`LaunchAgent`, der `open -a Diktator.app` ausführt, damit die Rechte der App gelten).
6. [x] Modell nach Leerlauf entladen (Speicher für LLM frei machen) – umgesetzt am 13.10.2026: Standard nach 30 min (`entladen_nach_minuten`, 0 = nie). Gemessen: 2,5 GB → 150 MB; das nächste Diktat lädt das Modell neu (≈ 1,2 s länger).
7. Silero-VAD statt Energieschwelle, falls Halluzinationen bei Pausen auftreten – auf der Beobachtungsliste (siehe Phase 1)
8. [x] Lokale Audio-/Videodateien transkribieren (gleiche Pipeline wie YouTube, z. B. Vorlesungsmitschnitte): Datei im Finder kopieren → Menüpunkt – umgesetzt am 13.10.2026: „Datei aus Zwischenablage – Deutsch/Englisch“ (Datei per ⌘C oder Pfad per ⌥⌘C), ffmpeg dekodiert, danach wie YouTube (Abschnitte, Vorrang für Diktate, Abbrechen, Absätze, Dateiname als Titel). Menüleiste zeigt `▶ 34 %` für beide Quellen.
9. [x] Einfügen mitten im Satz – umgesetzt und in Notizen getestet am 13.10.2026:
   - Swift-Hilfsprogramm `diktator-context` (`helpers/context.swift`, von `bundle.sh` in die App gebaut) liest je 40 Zeichen links und rechts der Schreibmarke über die Bedienungshilfen-API; die Rust-Bibliotheken können das nicht ohne `unsafe`. Antwortet es nicht binnen 800 ms oder fehlt (z. B. bei `cargo run`), wird eingefügt wie bisher.
   - Regeln (`src/text/fit.rs`): links ein Satz ohne Satzende → erstes Wort klein (Deutsch nur Funktionswörter wie „der“, „und“, „weil“, damit Nomen groß bleiben; „Sie“ bleibt; Englisch alles außer „I“ und Abkürzungen); nach „„“ bleibt es groß (wörtliche Rede). Leerzeichen nach links ergänzen. Rechts geht der Satz klein weiter oder steht ein Satzzeichen → Schlusspunkt weg.
   - Das Hilfsprogramm fragt die vordere App direkt (nicht das systemweite Fokus-Element, darauf antworten viele Apps nicht) und schaltet bei Chrome/Electron `AXManualAccessibility` ein. Scheitert es, steht der Grund im Protokoll.
   - Das Protokoll zeigt je 12 Zeichen links/rechts und das Ergebnis (zum Nachvollziehen; kann später raus).
   - E-Mail-Adressen (`src/text/email.rs`, in beiden Sprachen): Cohere schreibt „peter.hase at iCloud.com“ oder „Peter punktmeier at icloud punkt com“ → `peter.hase@icloud.com`. Nur wenn hinter „at“/„ät“ eine Endung folgt; „visit OpenCourseWare at ocw.mit.edu“ und „look at google.com“ bleiben. Im Alltag klappt es eher nicht, weil das Modell Adressen zu uneinheitlich erkennt – wichtige Adressen besser tippen oder einfügen; Regel bleibt als Hilfe drin.
10. [x] Zwischenablage als „vorübergehend“ markieren, damit Clipboard-Manager das Transkript nicht speichern – umgesetzt am 13.10.2026: Diktat und der danach wiederhergestellte alte Inhalt tragen `org.nspasteboard.ConcealedType` (nspasteboard.org, über arboard `exclude_from_history`); geprüft, dass der Typ in der Zwischenablage steht. Bewusst Kopiertes (YouTube-/Datei-Transkript, Verlauf) bleibt normal.
11. ~~Optional: Enter nach dem Einfügen (für Chats wie Claude)~~ – gestrichen (13.10.2026)
12. ~~Adressbuch für E-Mail-Adressen: gesprochener Name → Adresse (z. B. „Mail Peter“ → `peter.hase@icloud.com`), zuverlässiger als das Zusammensetzen gesprochener Adressen. Bis dahin geht es über `[ersetzungen]` im Wörterbuch.~~ – gestrichen (13.10.2026)

### Phase 4 – LLM-Nachbearbeitung

**Stand 13.10.2026 – erster Wurf mit Apple Foundation Models:** fn + ⌥ (mit ⇧ in der zweiten Sprache) diktiert wie sonst, sammelt aber die Teile und lässt den ganzen Text danach von Apples Modell korrigieren (`helpers/llm.swift` → `diktator-llm`, `src/polish.rs`); oben steht „Wird überarbeitet …“, Symbol ✨. Schlägt es fehl (Apple Intelligence aus, abgelehnt, zu lang, > 90 s), wird der rohe Text eingefügt. Anweisung auf Englisch mit zwei Beispielen und ausdrücklicher Sprache – mit deutscher Anweisung übersetzte das Modell englischen Text ins Deutsche, ohne Beispiele verdoppelte es Wörter oder ließ Spanisch unkorrigiert. Dauer ~3,5–5 s. Kleine Freiheiten bleiben („hungrig war“ statt „Hunger hatte“). Nächster Schritt: im Alltag testen, dann Anwendungsfälle festlegen.

**Sicherheitsnetz und lokales Modell (13.10.2026):** Erster Alltagstest: Ergebnis sieht gut aus, aber Apples kleines Modell (~3B) baut Logiklöcher und Einschübe ein. Darum wird die Antwort jetzt Wort für Wort mit dem Diktat verglichen (`text/guard.rs`, `similar` + `strsim`): übernommen werden nur Schreibung, Satzzeichen, kleine Schreibfehler im Wort (Levenshtein ≤ 1/2/3 je nach Länge) und Zusammen-/Getrenntschreibung; eingefügte, gestrichene oder ausgetauschte Wörter fallen weg, das Diktat bleibt. Zweite Wahl: lokales Modell über die OpenAI-kompatible Schnittstelle von LM Studio oder Ollama (`ureq`), einstellbar im Panel (Mit: Apple Intelligence / Lokales Modell, Programm, Modell aus der Liste des Servers); `<think>`-Abschnitte werden entfernt. Kandidaten (Recherche 13.10.2026): auf dem jetzigen Mac Gemma 4 12B (4 Bit, ~7–8 GB); auf einem neuen Mac mit ≥ 32 GB Gemma 4 26B-A4B, Qwen3.8 27B oder Mistral Small; Aleph Alpha Kolibri (78B MoE, Deutsch/Englisch) beobachten. Mit einem Ersatz-Server geprüft (Modellliste, Antwort, Netz verwirft Einschübe); mit echtem LM Studio noch nicht.
Einstellungen danach aufgeräumt: ein Block „Modelle“ mit Spracherkennung (Stufe nur als Auswahl, wenn mehrere Dateien da sind), Entladen und Überarbeiten (Apple/lokal, Programm, Modell); Info zeigt beide Modelle.
Ist das lokale Programm nicht erreichbar (nicht installiert/gestartet), weicht Diktator auf Apple Intelligence aus und meldet das („Ollama läuft nicht … – mit Apple Intelligence überarbeitet“) statt roh einzufügen. Rechtschreibschutz (umgesetzt 13.10.2026): Parallel zum Modell prüft `diktator-spell` (NSSpellChecker) die Wörter des Diktats. Was schon richtig geschrieben ist, darf das Modell nicht verändern („Hause“ → „Haus“, „was“ → „etwas“ werden verworfen); Satzzeichen und Großschreibung des Modells werden trotzdem übernommen. Ausnahme: eine Liste leicht verwechselter Wörter (das/dass, seid/seit, den/dem, to/too, then/than …). Ein richtig geschriebenes Wort wird auch nicht mehr auseinandergerissen. Sprachen ohne macOS-Rechtschreibprüfung (Chinesisch, Japanisch) laufen ohne diesen Schutz.

**Lokales Modell über llama.cpp statt LM Studio/Ollama (13.10.2026):** Beide Programme wollten wir nicht (Oberfläche, Account-Abfragen). Jetzt: `brew install llama.cpp`, GGUF-Datei von Hugging Face nach `models/llm/` (Gemma 4 12B, `unsloth/gemma-4-12B-it-qat-GGUF`, `UD-Q4_K_XL`, 6,7 GB, Apache-2.0, ohne Account). Diktator startet `llama-server` selbst beim ersten fn + ⌥ (nur `127.0.0.1`, freier Anschluss, `--offline`, ohne Nachdenk-Phase), beendet es nach der eingestellten Pause (Standard 10 min, im Panel wählbar wie bei der Spracherkennung), beim Wechsel zu Apple und beim Beenden; einen nach Absturz übrig gebliebenen Server räumt der nächste Start ab (`llama-server.pid`). Panel: Überarbeiten = Apple Intelligence oder eines der Modelle aus `models/llm/` (Name aus der Datei, „Gemma 4 12B“), darunter „Entladen nach Pause“ mit Speicherbedarf; Hinweise, wenn llama.cpp oder ein Modell fehlt. `adresse` in `settings.toml` entfällt (wird überlesen). Mit einem Mini-Modell (Qwen2.5 0.5B) geprüft: Start, Antwort (2. Aufruf 0,3 s), Entladen nach Pause, Neustart, Beenden, Aufräumen nach Absturz. Mit Gemma 4 12B im Alltag (13.10.2026): Ergebnis gut; Laden ~10 s, danach ~1–4 s je Diktat (22 Tokens/s). Einmal geriet Gemma bei 71 Wörtern in eine Wiederholung und schrieb bis zur Zeitgrenze (120 s, 2.500 Tokens) – jetzt ist die Antwort auf etwa das Doppelte des Diktats begrenzt (`max_tokens`), wird sie abgeschnitten, gilt sie als gescheitert (Ausweichen auf Apple).

- Eigene Taste (Vorschlag: fn + ⌥ für DE, fn + ⌥ + ⇧ für EN); die normale Taste bleibt roh und schnell.
- Zwei Wege, beide ohne Kosten:
  - **Apple Foundation Models** (auf macOS 27 vorhanden, läuft auf dem Gerät): Das Framework gibt es nur für Swift, also ein winziges Swift-Hilfsprogramm, das Text über stdin/stdout bekommt und zurückgibt. Kein `unsafe` in Rust nötig. Erste Wahl zum Ausprobieren, weil nichts heruntergeladen werden muss.
  - **Lokales Open-Weights-Modell** über eine OpenAI-kompatible Schnittstelle (LM Studio oder `llama-server`), z. B. Qwen3-8B in 4 Bit (~5 GB). Auf 16 GB knapp, auf dem neuen Mac bequem.
- Prompts als Dateien in `prompts/`, je Anwendungsfall eine Datei; Kandidaten: Grammatik glätten ohne Umformulieren, E-Mail, Stichpunkte/Gliederung, Karteikarte im Subsumo-Format, YouTube-Zusammenfassung.
- Prompts schreiben wir, wenn feststeht, wofür du sie nutzt.

### Phase 3.1 – Wörterbuch und Jura-Regeln (umgesetzt)

**Grundlage:** Was Cohere aus gesprochenen Normen macht (8 Testsätze mit der Systemstimme „Anna“ und das 75-min-BGB-Video):
- „Paragraf“ wird mal `§`, mal `Paragraph`, oft **gar nicht** geschrieben („Nach 433 Absatz 2 Satz 1 BGB“) – auch Sprecher lassen es oft weg.
- Zahlen kommen als Ziffern; Gesetze als `St. G. B.`, `VWGO`; `Erste Alternative`, `Analog` großgeschrieben; Kommas zwischen Absätzen fehlen.

**Stil:** Standard ist die Klausurschreibweise aus Subsumo (`§ 812 I 1 Alt. 1 BGB`, `Art. 2 I i.V.m. Art. 1 I GG`, `Nr.`, `Hs.`, `lit.`, `Var.`, `ff.`, `analog`) – dort ~19.000× römisch gegenüber 68× „Abs.“. Umschaltbar auf `§ 812 Abs. 1 S. 1 Alt. 1 BGB` (`absatz = "abgekürzt"`).

**Regel für nackte Zahlen:** Eine Zahl ohne `§`/„Paragraf“/„Artikel“ davor wird nur zur Norm, wenn ein Normteil (Absatz, Satz, Nummer, Fall …) oder ein Gesetz folgt. „50 Euro“, „60 bis 90 Minuten“, „Thema 1“ bleiben. Ob `§` oder `Art.`, entscheidet das Gesetz (GG, EMRK, AEUV, EGBGB … → `Art.`).

**Prüfung:** Im ganzen BGB-AT-Transkript (12.343 Wörter) ~60 Zitate umgeformt, kein Fehltreffer; das englische Transkript bleibt unverändert. 22 neue Unit-Tests mit rund 50 Beispielsätzen in `src/text/`.

**Bedienung:** Menü „Wörterbuch öffnen“ öffnet `~/Library/Application Support/Diktator/dictionary.toml` (wird mit Erklärungen angelegt). Änderungen gelten ab dem nächsten Diktat; fehlerhafte Datei → Mitteilung, alte Regeln gelten weiter. Ausprobieren ohne App: `echo "Paragraf 433 Absatz 2 BGB" | cargo run --example rules`.

Wirkt bei Diktat und YouTube; Jura-Regeln nur bei Deutsch, Ersetzungen bei beiden Sprachen.

**Zahlen als Ziffern (13.10.2026):** Cohere schreibt Zahlen mal als Ziffern, mal als Wörter („2,5 GB“ / „zwei Komma fünf Gigabyte“) – das Modell entscheidet nach Zusammenhang, ohne feste Regel. Neue Regel für deutsche Diktate und Transkripte (`text/figures.rs`, nach den Jura-Regeln): Kommazahlen, Zahlen vor Maßeinheiten/Währungen und Zahlen ab 13 werden Ziffern; kleine Zahlen ohne Einheit, „hundert“/„tausend“ allein und „ein“ bleiben Wörter.

### Phase 3.2 – Sprachbefehle (14.10.2026, erster Schritt)

Umgesetzt (`src/text/commands.rs`, nur bei Diktaten, vor Wörterbuch und Jura-Regeln):
- „neuer Absatz“ / „new paragraph“ → Leerzeile; der Text danach beginnt groß.
- „neue Zeile“ / „new line“ → Zeilenumbruch.
- Satzzeichen, die Cohere um den Befehl setzt („… gut. Neuer Absatz. Morgen …“, „…, neue Zeile, …“), fallen weg; ein Satzende davor bleibt.
- Folgt eine Zahl („neuer Absatz 2“), ist es kein Befehl (Normzitat).
- Ein Umbruch am Anfang oder Ende eines Diktats bleibt erhalten (vorher hat das Kürzen von Leerraum ihn verschluckt).

Im Alltag prüfen, ob Cohere die Befehle zuverlässig schreibt. Wenn es gut klappt, erweitern – Kandidaten:
„Punkt“, „Komma“, „Fragezeichen“, „Doppelpunkt“ als Satzzeichen (nur wenn das Modell sie nicht ohnehin setzt),
„Anführungszeichen auf/zu“, „Klammer auf/zu“, „Spiegelstrich“ / „neuer Punkt“ für Listen, „letzten Satz löschen“.
Eigene Befehle könnten dann im Wörterbuch stehen (`[befehle]`), so wie heute die Ersetzungen.

Ausprobiert und wieder entfernt (16.10.2026): **Befehle zum Bearbeiten** – ein Diktat, das nur aus einem
Befehl besteht („fett“, „kursiv“, „Wort markieren“, „Zeile markieren“, „Überschrift 1“, „Aufzählung“ …),
schickte statt Text das Tastenkürzel an die App (⌘B, ⌥⇧←, ⌘⌥1 …), gedacht vor allem für Google Docs.
Bewusst nicht übernommen. Falls es wiederkommt: Es steht vollständig im Commit `894e913`; Kürzel mit Z (⌘Z)
gehen so nicht, weil eine feste Taste gesendet wird und Z auf deutscher und englischer Tastatur an
verschiedenen Stellen liegt.

### Aufräumen und neue Funktionen (14.10.2026)

**Code-Durchsicht und Umbau** (keine sichtbare Änderung, aber nach den Richtlinien der Cargo.toml):
Reste aus einem Dioxus-Projekt aus der Cargo.toml entfernt; ein gemeinsames Modul für die
Swift-Hilfsprogramme (`helper.rs`, liest Ausgaben während des Laufs); Fehler-Enums statt Strings;
getypte Antwort des lokalen Modells; Überarbeiten als eigener Thread im Ordner `polish/`, der den
llama-server besitzt (vorher globaler Zustand mit Wächter-Thread); das laufende Diktat ist Teil des
Zustands der Steuerung; Steuerung aufgeteilt (`controller/dictation.rs`, `controller/transcript.rs`) und
über die Schnittstelle `Platform` mit nachgebautem Mac testbar; Textbearbeitung in fester Reihenfolge an
einer Stelle (`text::clean_dictation`, `text::clean_transcript`).

**Neu im Alltag:**
- Modelle laden schon beim Drücken von fn (Spracherkennung nach dem Entladen, bei fn + ⌥ das lokale Sprachmodell).
- fn nimmt schon auf, während das Modell nach dem Start noch lädt.
- Esc während des Überarbeitens fügt sofort den rohen Text ein.
- Erfundene Teile werden über die Sprechzeit erkannt (mehr als 30 Zeichen pro Sekunde Sprache → verworfen).
- Protokoll mit Datum und Ortszeit, ohne Text aus dem Diktat.
- Immer das eingebaute Mikrofon, auch wenn macOS auf AirPods oder das iPhone wechselt.
- Klänge und Entladezeiten gelten sofort; Neustart nur noch für Taste und Modelldatei.
- Datei-Transkripte zusätzlich als `.txt` neben der Datei (nichts wird überschrieben).
- Zeitmarken (`[12:30]`) vor jedem Absatz von YouTube- und Datei-Transkripten, Schalter im Panel.
- Drag & drop: Datei aufs Menüleisten-Symbol ziehen öffnet den Reiter „Datei“; dort auf eine Sprache ziehen.
- Regeln je App: Terminals und Chats ohne Schlusspunkt, Mail-Programme mit vollständigen Sätzen
  (Listen der Bundle-IDs in `src/text/fit.rs`). Eigene Regeln je App: siehe „Offene Ideen“, Punkt 3.
- Sprachbefehle „neuer Absatz“ / „neue Zeile“ (siehe Phase 3.2).
- Hinweis an der Schreibmarke: schwebender Punkt mit Pegel während der Aufnahme. Die Position kommt über
  die Bedienungshilfen (`AXBoundsForRange`); Apps, die sie nicht verraten, bekommen ihn am Mauszeiger.

**Zweite Runde:**
- Wächter: Diktator läuft als Kindprozess eines kleinen Wächters, der es nach einem Absturz neu startet
  (höchstens dreimal in zwei Minuten); „Jetzt neu starten“ geht denselben Weg (`src/watchdog.rs`).
- GitHub Actions prüft Formatierung, Clippy, Tests, Doku, Swift-Typprüfung (`.github/workflows/ci.yml`) – seit 14.10.2026 nur von Hand gestartet, nicht mehr bei jedem Push.
- Fachbegriffe im Wörterbuch (`[fachbegriffe]`, `src/text/terms.rs`): ähnlich erkannte Wörter werden zum Begriff.
- YouTube-Kapitel werden im Transkript zu Überschriften (`## Titel`).
- Mehrere Dateien auf einmal: Warteschlange, jede Datei bekommt ihre `.txt`; „Abbrechen“ verwirft die wartenden.

Im Alltag prüfen: Hinweis an der Schreibmarke in Notizen, Mail, Safari, Chrome, Claude, Terminal;
Drag & drop aufs Menüleisten-Symbol (auch mehrere Dateien); Regeln je App in Nachrichten/WhatsApp und Mail;
Sprachbefehle; Fachbegriffe; Kapitel bei einem Video mit Kapiteln; dass die Rechte (Mikrofon, Bedienungshilfen,
Eingabeüberwachung) auch für den Kindprozess des Wächters gelten.

### Teile, englische Oberfläche, Ablagefläche, leise Sprache (16.10.2026)

- **Schieber „Text beim Sprechen“** (Einstellungen → Diktat, `[diktat] teile_ab_sekunden`): Grund war, dass
  die Teile oft mitten im Satz geschnitten wurden und dort die Satzzeichen litten. Stufen 5, 10, 15, 20 s und
  „am Ende“; die eingestellte Zeit ersetzt die 5 s, die übrigen Grenzen wachsen mit (das Doppelte: jede
  Sprechpause, das Fünffache: auch ohne Pause), aber ein Teil ist nie länger als 28 s – längere teilte das
  Modell (höchstens 30 s am Stück) selbst noch einmal an beliebiger Stelle; deshalb gibt es keine Stufen über
  20 s. „Am Ende“: Diktate bis 30 s sieht das Modell ganz, längere teilt es an leisen Stellen.
  `chunk::PartTiming`, gilt sofort, auch für eine laufende Aufnahme.
- **Englische Oberfläche:** Schalter „English“ oben im Reiter Info (`[oberflaeche] sprache`). Das Panel kennt
  beide Sprachen (`model.t(…)`), Diktator schickt jede Meldung zweisprachig (`src/note.rs`), so wechselt auch
  eine schon angezeigte Meldung mit. Auch Fehlermeldungen gibt es in beiden Sprachen: Jeder Fehlertyp hat
  seine deutsche Meldung in `Display` und die englische über `note::Explain`; die Swift-Hilfsprogramme melden
  Fehler als `{"de": …, "en": …}`. Mitteilungen von macOS und der Titelzusatz übersetzter Transkripte
  („(aus dem Spanischen)“ / „(translated from Spanish)“) folgen der gewählten Sprache. Was andere Programme
  und Bibliotheken melden (macOS-Dateifehler, ffmpeg, yt-dlp), steht so da, wie es kommt – meist englisch.
  Deutsch bleibt nur das Protokoll.
- **Große Ablagefläche** im Reiter Datei: gestrichelt, beim Drüberziehen blau mit springendem Symbol
  („Loslassen zum Übernehmen“), danach mit Dateiname und ×; die Zeile „Datei transkribieren“ leuchtet beim
  Drüberziehen ebenfalls auf (dort losgelassen startet es sofort). `ImageRenderer` (`--snapshot`) zeichnet
  Ablageflächen und Schalter nur als Platzhalter – das Aussehen beim Ziehen also im Alltag prüfen.
- **Leise Sprache:** Die Untergrenze der Sprachschwelle (`MIN_THRESHOLD`) ist 0,0015 statt 0,004. Gemessen
  an einer verworfenen Aufnahme: leise Sprache 0,002–0,005, Grundrauschen 0,0002–0,0003; der Satz („Froh,
  keinen Hunger zu haben.“) wurde bei 0,004 verworfen, bei 0,0015 vollständig erkannt (bei 0,002 fehlte der
  Anfang). Die Stille dahinter ergab im Modell einen erfundenen Satz („Das ist ein sehr guter Punkt.“) –
  ohne Schwelle geht es also nicht; sie blieb auch bei 0,0015 unter beiden Grenzen (7 statt 8 Sprachrahmen,
  längste Folge 4 statt 5). Die zuletzt verworfene Aufnahme liegt als `letzte-leere-aufnahme.wav` im Ordner
  `Application Support/Diktator` zum Nachmessen.
- **Werte vom Panel werden beim Lesen geprüft:** Lautstärke (`feedback::Volume`, 0–100) und
  `teile_ab_sekunden` sind Typen, die Zahlen außerhalb ablehnen – vorher konnte ein solcher Wert gespeichert
  werden und die Datei beim nächsten Start unlesbar machen.

Im Alltag prüfen: Schieber (bringen 10–20 s bessere Zeichensetzung?), Ablagefläche beim Drüberziehen,
englische Oberfläche, ob bei der neuen Schwelle in Stille erfundene Sätze auftauchen.

### Offene Ideen (Stand 16.10.2026)

Vorgeschlagen, noch nicht umgesetzt – grob nach Nutzen sortiert:

1. ~~**Halten zum Sprechen:** fn gedrückt halten nimmt auf, bis die Taste losgelassen wird (heute zählt nur
   Antippen; ein Druck über 1,5 s löst nichts aus). `TapDetector` in `src/hotkey.rs` kennt die Dauer schon.~~
   – gestrichen (14.10.2026): Antippen ist bewusst so gewollt.
2. **Ersetzung aus dem Verlauf anlegen:** Klick auf ein Diktat im Verlauf → „falsch erkannt → richtig“ →
   Eintrag in `dictionary.toml`. So wird beim Diktieren nebenbei korrigiert.
3. **Regeln je App im Panel:** „ohne Schlusspunkt“ / „vollständige Sätze“ / „angepasst“ für beliebige Apps
   selbst einstellen (heute feste Listen in `src/text/fit.rs`); die App vorn meldet `diktator-context` schon.
4. ~~**Überwachter Ordner:** Neue Aufnahmen in einem gewählten Ordner (Vorlesungs-Rekorder, Sprachmemos) werden
   von selbst transkribiert und als `.txt` daneben abgelegt – nutzt die Warteschlange für Dateien.~~
   – gestrichen (14.10.2026).
5. **Zeigen, was beim Überarbeiten korrigiert wurde:** Im Verlauf sichtbar machen, welche Änderungen das
   Sicherheitsnetz übernommen und welche es verworfen hat – hilft auch beim Vergleich Apple ↔ lokales Modell.
6. ~~**Untertitel (SRT/VTT):** Erst prüfen, ob Cohere über transcribe.cpp genaue Zeiten liefert
   (`TimestampKind`); die heutigen Zeitmarken sind je Abschnitt geschätzt und reichen dafür nicht.~~
   – gestrichen (14.10.2026).
7. [x] **Nur einen Bereich transkribieren** – umgesetzt am 14.10.2026: Felder „von“/„bis“ in den Reitern
   YouTube und Datei (`src/audio/range.rs`; `12:30`, `1:02:05`, nur Minuten; leer = Anfang bzw. Ende). Dateien:
   ffmpeg `-ss`/`-to` vor `-i`. YouTube: yt-dlp `--download-sections` lädt nur den Teil; ohne
   `--force-keyframes-at-cuts` kamen mehrere Sekunden zu viel, mit ihm beginnt er genau (1:00–1:30 → 30,0 s ab
   60,00 s nachgemessen). Liegt der Anfang hinter dem Ende (Videolänge aus yt-dlp), gibt es eine klare Meldung.
   Titel mit Bereich, Zeitmarken und Kapitel zählen ab dem Anfang des Originals (das Kapitel, in dem der Bereich
   beginnt, wird die erste Überschrift). Nur für eine einzelne Datei; die Felder leeren sich, sobald das
   Transkript startet. Im Alltag geprüft (14.10.2026): klappt.
8. [x] **YouTube-Video übersetzen** – umgesetzt am 14.10.2026: Zeile „YT-Video übersetzen“ mit Zielsprache
   (Standard Deutsch). Transkribiert in der Originalsprache, dann übersetzt der Überarbeiten-Thread mit dem
   gewählten Modell (`src/polish/translate.rs`). Zerlegt wird in Titel, Überschriften und Absätze; Zeitmarken und
   `## ` werden vorher abgetrennt und danach wieder davorgesetzt – so bleiben sie an ihrer Stelle, und die Stücke
   passen in Apples Kontextfenster. Scheitert ein Stück, bleibt es im Original; Abbruch kopiert das Original;
   Diktate gehen zwischen zwei Absätzen vor. Gemessen: Gemma 4 12B ~3 s, Apple ~5 s pro Absatz (Spanisch →
   Deutsch, sauber). Gemma braucht `--reasoning-budget 0` (startet Diktator so), sonst denkt es bis zur
   Längengrenze. Noch nicht für Dateien (dort fehlt die Originalsprache). Reiter YouTube danach neu
   gegliedert: „Transkript anpassen“ (Teil von–bis, Zeitmarken), „YouTube-Video transkribieren“ („YouTube-Video“ mit
   der Sprache von YouTube, „Sprache selbst wählen“ mit allen 14 Sprachen statt fester Knöpfe für Deutsch und
   die zweite Sprache), „YouTube-Video übersetzen“. Reiter Datei genauso: „Transkript anpassen“ oben, darunter
   „Datei transkribieren“ mit Ablagefläche und Sprachauswahl (alle 14). Texte im Panel einheitlich: ganze Sätze
   und Anweisungen mit Punkt, Stichworte, Aufzählungen, Beispiele und Befehle ohne. Im Alltag prüfen.

Dazu die Erweiterung der Sprachbefehle (Phase 3.2), wenn die ersten beiden im Alltag zuverlässig klappen –
ohne die Befehle zum Bearbeiten (16.10.2026 verworfen).

### Phase 5 – Veröffentlichung auf GitHub (zum Schluss)

Erst wenn Phase 3/4 abgeschlossen sind und Diktator im Alltag stabil läuft. Das Repository gibt es schon (`origin` = `github.com/HalmoBalmo2025/Dictator`, der Stand wird laufend hochgeladen; die Prüfung mit GitHub Actions startet vorerst nur von Hand); dort wird es öffentlich gemacht und bekommt Releases.

**Schritt 1 – Für Entwickler: gute Anleitung zum Selberbauen** (kaum Aufwand)
- README um einen Abschnitt „Selbst bauen“ erweitern, Schritt für Schritt für Leute, die das Terminal nur grob kennen:
  1. Werkzeuge: `xcode-select --install` (Compiler, `swiftc`, `codesign` – das volle Xcode ist nicht nötig), Rust über rustup (`curl https://sh.rustup.rs -sSf | sh`), Homebrew, dann `brew install cmake ffmpeg yt-dlp deno`. CMake braucht transcribe.cpp beim Bauen; ffmpeg, yt-dlp und Deno braucht Diktator zur Laufzeit (Dateien, YouTube).
  2. Repository holen: `git clone https://github.com/HalmoBalmo2025/Dictator.git && cd Dictator`.
  3. Modell herunterladen (curl-Befehl aus Abschnitt 6) nach `~/Library/Application Support/Diktator/models/`.
  4. Empfohlen: eigenes Zertifikat „Diktator Dev“ in der Schlüsselbundverwaltung anlegen (Anleitung aus dem Kopf von `scripts/bundle.sh`, mit Bildschirmfotos oder Klickpfad) – sonst fragt macOS nach jedem Neubau die Rechte neu ab.
  5. `./scripts/bundle.sh` (baut, installiert nach `~/Applications` und startet die App).
  6. Rechte erteilen (Mikrofon, Bedienungshilfen, Eingabeüberwachung) und die 🌐-Taste auf „Keine Aktion“ stellen – mit Verweis auf Info → „Rechte prüfen“ / „fn-Taste freigeben“ im Panel.
  7. Fehlersuche: häufige Fehler und was hilft (Modell fehlt, Rechte fehlen, `cargo` nicht gefunden, CMake fehlt, yt-dlp veraltet → `brew upgrade yt-dlp`).
- Voraussetzungen klar nennen: Apple Silicon, macOS 14 oder neuer (Apple Intelligence ab macOS 27), ≥ 8 GB Arbeitsspeicher (16 GB empfohlen).
- Lizenz für das Projekt wählen (z. B. MIT oder Apache 2.0) und `LICENSE` anlegen; Lizenz des Cohere-Modells prüfen und in der README nennen (Modell wird nicht mitgeliefert, jeder lädt es selbst).
- Vor dem Veröffentlichen prüfen, dass nichts Privates im Repository liegt (Pfade, Protokolle, Testaufnahmen, persönliche Wörterbuch-Einträge).

**Schritt 2 – GitHub Release mit fertiger App** (für alle ohne Entwickler-Werkzeuge)
- Ein Release = eine Versionsseite unter `…/Dictator/releases` mit Versions-Tag (z. B. `v0.1.0`), Beschreibung und angehängten Dateien (je höchstens 2 GB).
- Ablauf: Version in `Cargo.toml` erhöhen (`bundle.sh` übernimmt sie) → `./scripts/bundle.sh` → App als `Diktator-0.1.0.dmg` (oder `.zip`) verpacken → Release anlegen, entweder im Browser („Draft a new release“, Tag eingeben, Datei anhängen, „Publish“) oder mit der GitHub-Kommandozeile: `brew install gh`, `gh auth login`, dann `gh release create v0.1.0 Diktator-0.1.0.dmg --title "Diktator 0.1.0" --notes-file CHANGELOG.md`.
- Release-Text: was neu ist, Installation (App in „Programme“ ziehen, `brew install ffmpeg yt-dlp deno`, Modell wird beim ersten Start geladen), Hinweis auf „Trotzdem öffnen“ solange die App nicht notarisiert ist.
- Später automatisierbar mit GitHub Actions (macOS-Runner baut bei jedem neuen Tag die App und legt das Release an).

**Schritt 3 – Eigenständige App** (nur wenn es Interesse gibt)
- ffmpeg ersetzen durch macOS-eigenes Dekodieren (AVFoundation, Swift-Hilfsprogramm wie `diktator-context`): MP3, M4A, AAC, WAV, AIFF, FLAC, MP4, MOV, M4V; YouTube als M4A laden. Fallen weg: OGG, Opus, MKV, WebM, WMA (oder ffmpeg mitliefern, ~50–80 MB, Lizenz beachten).
- yt-dlp und Deno als Einzeldateien beim ersten Bedarf nach `Application Support` laden; yt-dlp regelmäßig selbst aktualisieren (`yt-dlp -U`), weil YouTube sich oft ändert.
- Modell beim ersten Start mit Fortschrittsbalken im Panel von Hugging Face laden (zu groß fürs Release und für die App).
- Signatur: Apple-Entwicklerprogramm (99 €/Jahr) → Developer-ID-Signatur und Notarisierung: App öffnet ohne Warnung, Rechte bleiben über Updates erhalten. Ohne: einmal „Trotzdem öffnen“ je Version und Rechte-Abfrage nach jedem Update.
- Optional: Update-Hinweis im Panel, wenn es bei GitHub ein neueres Release gibt.

## 8. Risiken

| Risiko | Gegenmaßnahme |
|---|---|
| `transcribe-cpp` ist jung (0.4, schnelle Versionssprünge) | Version in Cargo.lock festhalten; Phase 0 prüft zuerst. Ausweichen: `cohere_transcribe_rs` (MLX) |
| fn-Erkennung kollidiert mit macOS | Globus-Taste auf „Keine Aktion“; Taste später in `settings.toml` änderbar |
| Rechte gehen bei jedem Neubau verloren (lokale Signatur) | In Phase 0 prüfen; notfalls feste Signatur mit eigenem Zertifikat aus der Schlüsselbundverwaltung |
| Manche Apps nehmen ⌘V nicht (selten) | Phase 0 testet Notizen, Mail, Claude; notfalls App-Ausnahmen |
| YouTube ändert etwas | `brew upgrade yt-dlp` behebt es meist |
| Speicher (16 GB) mit Modell + Browser + später LLM | Q5_K_M-Stufe, Modell entladen (Phase 3) |
| Gemischte Sprache in einem Satz | Cohere ist dafür nicht gemacht; Sprache pro Aufnahme wählen |

## 9. Offene Punkte

- 14.10.2026: die neuen Funktionen im Alltag prüfen (Liste unter „Aufräumen und neue Funktionen“, Phase 3.2 Sprachbefehle).
- 16.10.2026: Schieber, englische Oberfläche, Ablagefläche und die neue Sprachschwelle im Alltag prüfen (Liste unter „Teile, englische Oberfläche, Ablagefläche, leise Sprache“).
- Nächste Schritte: siehe „Offene Ideen (Stand 16.10.2026)“ in Abschnitt 7.
- Phase 1: Abnahme „eine Woche Alltag“.
- Oberfläche (13.10.2026): Panel statt Menüliste, nach dem Vorbild von Vorssaint – Wellenform-Symbol statt `◦`, Panel mittig unter dem Symbol, Reiterleiste mit Symbolen (Diktat, YouTube, Datei, Wörterbuch, Einstellungen, Info), Karten mit Zeilen, Beenden als ⏻ rechts in der Reiterleiste. Korrekturen nach dem ersten Test: Panel wuchs nach oben über die Menüleiste (Fenstergröße folgte dem Inhalt nicht) → `sizingOptions = .preferredContentSize`; Mikrofon bei Aufnahme orange; kurze Mikrofon-Aussetzer (`Xrun`) verwerfen die Aufnahme nicht mehr, sie werden nur gezählt und protokolliert. `diktator-ui --open` zeigt das Panel sofort (zum Prüfen der Lage). Zweite Runde: deckender Hintergrund statt Glas (Lesbarkeit), Mikrofon rot, Kopf kompakt (Symbol links, Zustand und Meldung rechts, feste Höhe), Meldung wird beim Start einer Aufnahme geleert, Menüleisten-Symbol mit fester Breite (Panel springt nicht mehr), Prozent nur noch im Panel. Dritte Runde: Symbol quadratisch wie alle anderen (kein „DE“ mehr in der Leiste); Einstellungen und Wörterbuch als Bedienelemente im Panel (Schalter, Auswahl, Schieberegler, Ersetzungsliste, Kürzel), Diktator schreibt die TOML-Dateien samt Erklärungen neu; Neustart-Hinweis (orange, „Jetzt neu starten“) nur, wenn die Datei von den laufenden Einstellungen abweicht; „Protokoll löschen“; bei offenem Panel geht ein Diktat in die Zwischenablage. `@State` geht mit `swiftc` ohne Xcode nicht (Makro-Plugin fehlt) – Eingaben liegen deshalb im Model. Rust schickt Zustand/Notiz/Verlauf/Fortschritt als JSON-Zeilen, das Panel schickt Befehle zurück (`src/app/ui.rs`). `diktator-ui --snapshot <ordner>` zeichnet alle Reiter hell und dunkel als PNG. Im Alltag prüfen.
- Zweite Sprache (13.10.2026): Cohere kennt laut Modelldatei 14 Sprachen (en, fr, de, es, it, pt, nl, pl, el, ar, ja, zh, vi, ko). ⇧ fn diktiert in der im Panel gewählten zweiten Sprache (`[sprachen] zweite`, Standard Englisch); gilt sofort ohne Neustart. YouTube/Datei bieten „auf Deutsch“ und „auf <zweite Sprache>“; automatisch nimmt jede vom Modell gekannte YouTube-Sprache (sonst Englisch). Jura-Regeln nur Deutsch; Kleinschreibung mitten im Satz für andere Sprachen wie Englisch; Japanisch/Chinesisch ohne Leerzeichen davor. Spanisch mit Systemstimme geprüft. Danach: Einstellungen neu geordnet (Diktat: zweite Sprache + Taste · Klänge · Allgemein · Modell), Hinweis „Mit **fn ⇧**“; Info → „fn-Taste freigeben“ öffnet die Tastatur-Einstellungen (🌐-Taste auf „Keine Aktion“); Panel sprang bei Aufnahme ein paar Punkte nach unten, weil das Mikrofon-Symbol anders bemessen ist – alle Symbole jetzt auf gleich großer Fläche, leere Meldung behält ihre Höhe (per Bildschirmfoto nachgemessen). Feinschliff: Diktat-Reiter zeigt „fn ⇧“, Verlauf ohne Zähler; YouTube: Deutsch → zweite Sprache → automatisch, während ein Transkript läuft sind die Knöpfe ausgegraut und „Läuft“ mit Fortschritt steht darunter; Info ohne „Taste“, eigener Block „Protokoll“ mit Schalter (`[protokoll] an`, gilt sofort), Öffnen/Löschen nur bei eingeschaltetem Protokoll. Reiter Datei: Formate aufgeführt (alle 15 mit einer Testaufnahme durch ffmpeg geprüft), beim Lesen steht der Dateiname im „Läuft“-Block, unbestimmter Fortschritt als wandernder Balken. Texte im Panel mit bewussten Zeilenumbrüchen statt Umbruch mitten im Satz, „gilt sofort“ entfernt (der orange Hinweis zeigt, wann ein Neustart nötig ist), settings.toml unter Info, Wörterbuch mit grauem Knopf „Datei öffnen“.
- „Herr Präsident!“ am Diktatende (13.10.2026): Der Klick der fn-Taste zählte als Sprache, dadurch blieb die Pause davor in der Aufnahme, und in Stille erfindet Cohere einen Satz (offenbar aus Parlamentsreden gelernt). Jetzt zählen am Rand nur Sprachfolgen ab 100 ms (`MIN_EDGE_RUN`); nachgestellt: Satz + 2 s Pause + Klick → 3,7 s werden zu 1,7 s, Transkript sauber. Zusätzlich fällt ein allein stehender Schlusssatz „Herr Präsident!“/„Frau Präsidentin!“ bei Diktaten weg (`text/phantom.rs`).
- Diktat in Teilen (13.10.2026): Während der Aufnahme schneidet Diktator nach ≥ 10 s an der ersten Sprechpause (≥ 300 ms, spätestens nach 25 s am leisesten Rahmen) einen Teil ab (`chunk::part_end`, `Recording::next_part`, Takt alle 0,5 s), das Modell transkribiert ihn, und er wird sofort eingefügt; die Anpassung an die Schreibmarke greift, weil der vorige Teil links davon steht. Jeder Teil trägt die Nummer des Diktats (`PartId`), Ergebnisse verworfener Diktate (Esc) werden ignoriert; schon eingefügter Text bleibt. Bei offenem Panel wird gesammelt und am Ende kopiert; im Verlauf steht ein Eintrag je Diktat. Simuliert mit 40 s Sprache: Schnitte genau an den Satzpausen nach ~13 s, Text vollständig. Erster Alltagstest: Springen zwischen Apps klappt; aber in Mail fehlte die Umgebung (Verfassen-Fenster ist eine Webseite, `AXWebArea` ohne Schreibmarke) → Teile klebten mit Punkt und Großbuchstaben aneinander; im Terminal kamen Bildschirmrahmen und Nullzeichen als Umgebung. Jetzt: Ist die Umgebung nicht lesbar, gilt das Ende des zuletzt eingefügten Teils als Text links der Schreibmarke; Terminals werden gar nicht gelesen; Nullzeichen werden Leerzeichen; Punkt fällt nur weg, wenn rechts ein Buchstabe oder eine Ziffer klebt. Nicht weiter verfolgt: ersten Teil in Mail an die Umgebung anpassen. Versuch kürzere Teile: ab 5 s wird an einer langen Pause (≥ 600 ms, meist Satzende) geschnitten, ab 10 s an jeder Sprechpause (≥ 300 ms), spätestens nach 25 s; simuliert ergibt das je Satz einen Teil (6–8 s). Ergebnis im Alltag: passt für normales Diktieren; eine lange Denkpause mitten im Satz endet mit einem Punkt – bleibt so. (Betrifft nur Diktate; YouTube und Dateien werden weiter in Abschnitten bis 30 s an der leisesten Stelle geschnitten.)
- „Letztes Transkript kopieren“ gibt es je Quelle: der YouTube-Reiter kopiert das letzte YouTube-Transkript, der Datei-Reiter das letzte Datei-Transkript (vorher ein gemeinsames). Kopf des Panels: Zustand und Meldung mit gleichem Abstand oben, dazwischen und unten, ohne Meldung mittig; feste Höhe.
- Phase 3: Punkt 9 in Notizen, Mail, Claude und Terminal bestätigt; 4–6 bestätigt (13.10.2026). 8 und 10 im Alltag prüfen (Datei-Transkript über das Menü). Phase 3 abgeschlossen (7 auf der Beobachtungsliste, 11 und 12 gestrichen); Phase 4 läuft (Apple Foundation Models, erster Wurf zum Testen), zum Schluss Phase 5 (GitHub: Anleitung zum Selberbauen, Release, ggf. eigenständige App).
