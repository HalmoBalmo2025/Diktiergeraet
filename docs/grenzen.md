# Gemessene Grenzen und Werte

Was der Code nicht zeigt: Grenzen des Modells und Messwerte, auf die sich
Konstanten stützen. Der Code verweist hierher.

## Spracherkennung (Cohere Transcribe über transcribe.cpp)

- transcribe.cpp nimmt höchstens etwa 400 s Audio pro Aufruf; Cohere
  Transcribe ist auf Stücke von etwa 35 s trainiert und wird bei längeren
  ungenauer.
- Deshalb teilt Diktator selbst: Jede Aufnahme geht in Abschnitten von
  höchstens 30 s ans Modell (`MAX_CHUNK_SAMPLES` in `src/audio/chunk.rs`,
  geteilt am leisesten Rahmen im letzten Drittel). Das Modell teilt nichts.
- Ein Teil eines laufenden Diktats ist höchstens 28 s lang
  (`PART_LIMIT_SECONDS`): Der Abstand zu 30 s fängt ab, dass nur jede halbe
  Sekunde nachgesehen wird, so wird ein Teil nicht noch einmal geteilt.

## Mikrofon und Stille

- Eingebautes Mikrofon in einem ruhigen Raum: Grundrauschen (RMS je
  20-ms-Rahmen) 0,0002 bis 0,0003, leise Sprache 0,002 bis 0,005. Bei einer
  Schwelle von 0,004 gingen leise gesprochene Sätze verloren; Stille mit
  Tastenklicks blieb auch bei 0,0015 unter der Mindestzahl an
  Sprachrahmen (`MIN_THRESHOLD` in `src/audio/chunk.rs`).

## Dateien und YouTube

- Dekodieren mit ffmpeg: eine Aufnahme von drei Stunden braucht etwa eine
  Minute (`DECODE_TIMEOUT` in `src/audio/decode.rs` lässt 15 min).
- ffmpeg kappt beim Dekodieren nicht: AAC-Rauschen mit Amplitude 0,9 ergab
  Werte bis 1,05 und −1,08. `Pcm16k` kappt deshalb selbst auf [−1, 1].
- Ein Bereich mit `--force-keyframes-at-cuts` beginnt genau: 1:00–1:30
  ergab 30,0 s ab 60,00 s. Ohne die Option schneidet yt-dlp am nächsten
  Schlüsselbild und liefert mehrere Sekunden zu viel (`section_args` in
  `src/youtube.rs`).
