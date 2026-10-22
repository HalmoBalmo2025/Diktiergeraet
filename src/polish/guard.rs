//! Sicherheitsnetz fürs Überarbeiten: Von der Antwort eines Sprachmodells
//! wird nur übernommen, was nach Korrektur aussieht.
//!
//! Diktat und Antwort werden Wort für Wort verglichen. Übernommen werden
//! Groß-/Kleinschreibung, Satzzeichen, kleine Schreibfehler innerhalb eines
//! Wortes („dan“ → „dann“, „das“ → „dass“) und Zusammen- oder
//! Getrenntschreibung („los gegangen“ → „losgegangen“). Neu eingefügte,
//! gestrichene oder ausgetauschte Wörter werden verworfen; dort bleibt das
//! Diktat stehen. So kann das Modell nichts erfinden und nichts weglassen.
//!
//! Zeilenumbrüche und Leerzeilen („neue Zeile“, „neuer Absatz“) und aller
//! Leerraum zwischen den Wörtern kommen aus dem Diktat, auch am Anfang und
//! am Ende.
//!
//! Wörter, die schon richtig geschrieben sind (laut Rechtschreibprüfung),
//! bleiben ebenfalls – „nach Hause“ wird nicht zu „nach Haus“. Ausnahme
//! sind Wörter, die man leicht verwechselt („das“ → „dass“, „to“ → „too“).
//! Konnte nicht geprüft werden, gilt jedes Wort als richtig geschrieben.
//! Getrennt wird nur ein falsch geschriebenes Wort („i.V.m.“ → „i. V. m.“),
//! zusammengeschrieben nur, wenn eins der Wörter falsch geschrieben ist
//! („zu sagen“ bleibt). Dabei bleibt jedes Wort erhalten: „es geschaft“
//! wird nicht zu „geschafft“.
//!
//! Satzzeichen, die für sich stehen („–“, „?“ nach französischer Art),
//! kommen aus der Antwort; die des Diktats fallen weg, wenn das Modell sie
//! nicht übernimmt. Satzzeichen eines verworfenen Einschubs („, oder?“)
//! fallen mit ihm weg. Zeichen wie „&“ oder „%“ zählen als Wörter.
//!
//! Nie übernommen werden geänderte Zahlen („§ 433“ → „§ 434“, „2,5“ →
//! „25“) und gekippte Verneinungen („unwirksam“ → „wirksam“, „kein“ →
//! „ein“), auch wenn sie wie ein Schreibfehler aussehen.

use std::borrow::Cow;
use std::collections::BTreeSet;

use similar::{Algorithm, DiffOp, capture_diff_slices};

/// Richtig geschriebene Wörter, die trotzdem oft das falsche sind: Hier
/// darf das Modell das eine durch das andere ersetzen (Kerne, klein).
const CONFUSABLE: [(&str, &str); 33] = [
    ("das", "dass"),
    ("seid", "seit"),
    ("wieder", "wider"),
    ("tot", "tod"),
    ("war", "wahr"),
    ("man", "mann"),
    ("wen", "wenn"),
    ("den", "denn"),
    ("dem", "den"),
    ("einem", "einen"),
    ("ihm", "ihn"),
    ("ihn", "in"),
    ("ihm", "im"),
    ("viel", "fiel"),
    ("statt", "stadt"),
    ("ist", "isst"),
    ("mal", "mahl"),
    ("to", "too"),
    ("then", "than"),
    ("their", "there"),
    ("were", "where"),
    ("lose", "loose"),
    ("of", "off"),
    ("weather", "whether"),
    ("quite", "quiet"),
    ("advice", "advise"),
    ("affect", "effect"),
    ("accept", "except"),
    ("breath", "breathe"),
    ("choose", "chose"),
    ("lead", "led"),
    ("principal", "principle"),
    ("desert", "dessert"),
];

/// Was die Rechtschreibprüfung über die Wörter des Diktats weiß.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Spelling<'a> {
    /// Die Kerne ([`core()`]) der Wörter, die richtig geschrieben sind.
    Checked(&'a BTreeSet<String>),
    /// Nicht geprüft (Hilfsprogramm fehlt, Zeit abgelaufen, Sprache
    /// unbekannt): Jedes Wort gilt als richtig geschrieben.
    Unchecked,
}

/// Ergebnis des Abgleichs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Guarded {
    /// Der Text mit den übernommenen Korrekturen.
    pub(crate) text: String,
    /// Wörter, die das Modell eingefügt hätte (verworfen).
    pub(crate) dropped_insertions: usize,
    /// Wörter des Diktats, die das Modell gestrichen oder ausgetauscht hätte
    /// (behalten).
    pub(crate) kept_originals: usize,
    /// Richtig geschriebene Wörter, die das Modell verändert hätte
    /// (behalten).
    pub(crate) kept_correct: usize,
}

/// Ein Wort samt Satzzeichen, wie es im Text steht, und sein Kern zum
/// Vergleichen (klein, ohne Satzzeichen).
#[derive(Debug)]
struct Token<'a> {
    /// Der Leerraum davor, wie er im Text steht; beim ersten Wort leer.
    gap: &'a str,
    raw: &'a str,
    /// Leer genau dann, wenn das Token nur aus Satzzeichen besteht.
    core: String,
}

impl Token<'_> {
    /// Nur Satzzeichen ([`PUNCTUATION`]), kein Wort.
    const fn is_punctuation(&self) -> bool {
        self.core.is_empty()
    }
}

/// Satzzeichen: Was nur aus ihnen besteht, ist kein Wort.
const PUNCTUATION: &str = ".,;:!?…-‐‑‒–—―()[]{}\"'„“”‚‘’«»‹›¿¡";

/// Satzzeichen, die zum folgenden Wort gehören: Gedankenstriche, Klammern
/// auf, „ und ‚, ¿ und ¡.
const OPENING: &str = "-‐‑‒–—―([{„‚¿¡";

/// Ein Stück des Ergebnisses: ein Wort und der Leerraum davor.
#[derive(Debug)]
struct Piece<'a> {
    /// Leer heißt ein Leerzeichen (außer am Anfang des Textes).
    gap: &'a str,
    word: Cow<'a, str>,
    source: Source<'a>,
}

/// Woher ein Wort des Ergebnisses kommt.
#[derive(Debug, Clone, Copy)]
enum Source<'a> {
    /// Aus dem Diktat, weil das Modell es gestrichen oder ausgetauscht hätte.
    Dictation,
    /// Die Fassung des Modells für das Wort `dictated` des Diktats.
    Model { dictated: &'a str },
    /// Satzzeichen, die für sich stehen.
    Punctuation,
}

/// Übernimmt aus `polished` nur Korrekturen an `original`. `spelling`: was
/// die Rechtschreibprüfung über die Wörter von `original` weiß.
#[must_use]
pub(crate) fn keep_corrections(original: &str, polished: &str, spelling: Spelling<'_>) -> Guarded {
    let (lead, body) = split_off_whitespace(original);
    let (body, trail) = body.split_at(body.trim_end().len());
    let old = tokens(body);
    let new = tokens(polished.trim_start());
    let old_cores: Vec<&str> = old.iter().map(|token| token.core.as_str()).collect();
    let new_cores: Vec<&str> = new.iter().map(|token| token.core.as_str()).collect();
    let mut merger = Merger::new(spelling);
    let mut pending: Option<(usize, usize, usize, usize)> = None;
    for op in capture_diff_slices(Algorithm::Myers, &old_cores, &new_cores) {
        let (old_range, new_range) = (op.old_range(), op.new_range());
        if let DiffOp::Equal {
            old_index: _,
            new_index: _,
            len: _,
        } = op
        {
            if let Some((old_start, old_end, new_start, new_end)) = pending.take() {
                merger.region(sub(&old, old_start, old_end), sub(&new, new_start, new_end));
            }
            merger.take_all(
                sub(&old, old_range.start, old_range.end),
                sub(&new, new_range.start, new_range.end),
            );
        } else {
            pending = Some(match pending {
                Some((old_start, _, new_start, _)) => {
                    (old_start, old_range.end, new_start, new_range.end)
                }
                None => (
                    old_range.start,
                    old_range.end,
                    new_range.start,
                    new_range.end,
                ),
            });
        }
    }
    if let Some((old_start, old_end, new_start, new_end)) = pending {
        merger.region(sub(&old, old_start, old_end), sub(&new, new_start, new_end));
    }
    merger.finish();
    move_sentence_start(&mut merger.pieces);
    // Ging das letzte Wort der Antwort verloren, bleibt ihr Schlusszeichen.
    if let Some(end) = polished
        .trim_end()
        .chars()
        .last()
        .filter(|c| matches!(c, '.' | '!' | '?'))
        && let Some(last) = merger.pieces.last_mut()
        && !last.word.ends_with(['.', '!', '?', ':', ';', ','])
    {
        last.word.to_mut().push(end);
    }
    Guarded {
        text: render(lead, &merger.pieces, trail),
        dropped_insertions: merger.dropped_insertions,
        kept_originals: merger.kept_originals,
        kept_correct: merger.kept_correct,
    }
}

/// Teilstück einer Wortliste (leer, wenn der Bereich nicht passt).
fn sub<'t, 'a>(tokens: &'t [Token<'a>], start: usize, end: usize) -> &'t [Token<'a>] {
    tokens.get(start..end).unwrap_or_default()
}

/// Setzt den Text aus seinen Stücken zusammen, mit dem Leerraum `lead`
/// davor und `trail` danach.
fn render(lead: &str, pieces: &[Piece<'_>], trail: &str) -> String {
    let mut text = String::from(lead);
    for (index, piece) in pieces.iter().enumerate() {
        if index > 0 {
            text.push_str(if piece.gap.is_empty() { " " } else { piece.gap });
        }
        text.push_str(&piece.word);
    }
    text.push_str(trail);
    text
}

/// Baut den Ergebnistext auf.
#[derive(Debug)]
struct Merger<'a> {
    spelling: Spelling<'a>,
    pieces: Vec<Piece<'a>>,
    /// Die Satzzeichen am Ende des zuletzt verworfenen Einschubs, solange
    /// danach nichts ausgegeben wurde.
    after_insertion: Option<String>,
    /// Öffnende Satzzeichen ([`OPENING`]) des Modells, die auf ihr Wort
    /// warten: Wird es verworfen, fallen sie mit ihm weg.
    opening: Vec<&'a str>,
    dropped_insertions: usize,
    kept_originals: usize,
    kept_correct: usize,
}

impl<'a> Merger<'a> {
    /// Noch nichts zugeordnet.
    const fn new(spelling: Spelling<'a>) -> Self {
        Self {
            spelling,
            pieces: Vec::new(),
            after_insertion: None,
            opening: Vec::new(),
            dropped_insertions: 0,
            kept_originals: 0,
            kept_correct: 0,
        }
    }

    /// Gleiche Kerne: die Fassung des Modells (Schreibung, Satzzeichen)
    /// mit dem Leerraum des Diktats.
    fn take_all(&mut self, old: &[Token<'a>], new: &[Token<'a>]) {
        debug_assert_eq!(old.len(), new.len(), "invariant: gleiche Kerne paarweise");
        for (dictated, polished) in old.iter().zip(new) {
            if dictated.is_punctuation() {
                self.emit(Piece {
                    gap: dictated.gap,
                    word: Cow::Borrowed(polished.raw),
                    source: Source::Punctuation,
                });
            } else {
                self.take(dictated, polished.raw);
            }
        }
    }

    /// Die Fassung `word` des Modells für das Wort `old` des Diktats.
    fn take(&mut self, old: &Token<'a>, word: impl Into<Cow<'a, str>>) {
        self.emit(Piece {
            gap: old.gap,
            word: word.into(),
            source: Source::Model { dictated: old.raw },
        });
    }

    /// Ein Stück ausgeben. Davor wird erledigt, was noch offen ist: die
    /// Satzzeichen eines eben verworfenen Einschubs und öffnende
    /// Satzzeichen, die zu diesem Stück gehören.
    fn emit(&mut self, piece: Piece<'a>) {
        self.settle_insertion();
        for raw in self.opening.drain(..) {
            self.pieces.push(Piece {
                gap: "",
                word: Cow::Borrowed(raw),
                source: Source::Punctuation,
            });
        }
        self.pieces.push(piece);
    }

    /// Am Ende des Textes: erledigen, was noch offen ist.
    fn finish(&mut self) {
        self.settle_insertion();
        let opening: Vec<&'a str> = std::mem::take(&mut self.opening);
        for raw in opening {
            self.emit(Piece {
                gap: "",
                word: Cow::Borrowed(raw),
                source: Source::Punctuation,
            });
        }
    }

    /// Ein Satzzeichen, das das Modell für sich gesetzt hat: Direkt nach
    /// einem verworfenen Einschub gehört es zu ihm, ein öffnendes wartet
    /// auf sein Wort, jedes andere wird übernommen.
    fn punctuation(&mut self, raw: &'a str) {
        if let Some(marks) = &mut self.after_insertion {
            marks.push_str(raw);
        } else if raw.starts_with(|c: char| OPENING.contains(c)) {
            self.opening.push(raw);
        } else {
            self.emit(Piece {
                gap: "",
                word: Cow::Borrowed(raw),
                source: Source::Punctuation,
            });
        }
    }

    /// Ein Wort, das das Modell eingefügt hätte, verwerfen – mit den
    /// öffnenden Satzzeichen davor; die Satzzeichen danach merken.
    fn drop_insertion(&mut self, token: &Token<'a>) {
        self.dropped_insertions = self.dropped_insertions.saturating_add(1);
        self.opening.clear();
        let word = token
            .raw
            .trim_end_matches(|c: char| PUNCTUATION.contains(c));
        self.after_insertion = Some(token.raw.split_at(word.len()).1.to_owned());
    }

    /// Endete ein verworfener Einschub mit Satzzeichen, gehörte ein Komma
    /// (Semikolon, Doppelpunkt) am Wort davor zu ihm: „gut, oder?“ → „gut“,
    /// „komme, wenn es geht, morgen“ → „komme morgen“. Endete der Einschub
    /// einen Satz, endet ihn jetzt das Wort davor.
    fn settle_insertion(&mut self) {
        let Some(marks) = self.after_insertion.take() else {
            return;
        };
        let Some(last) = self.pieces.last_mut() else {
            return;
        };
        let word = last.word.trim_end_matches([',', ';', ':']);
        if marks.is_empty() || word.len() == last.word.len() {
            return;
        }
        let mut word = word.to_owned();
        if let Some(end) = marks.chars().rev().find(|c| matches!(c, '.' | '!' | '?')) {
            word.push(end);
        }
        if word.is_empty() {
            self.pieces.pop();
        } else {
            last.word = Cow::Owned(word);
        }
    }

    /// Ein Bereich, in dem sich Diktat und Antwort unterscheiden: der Reihe
    /// nach zuordnen, was als Korrektur durchgeht.
    fn region(&mut self, old: &[Token<'a>], new: &[Token<'a>]) {
        let (mut old, mut new) = (old, new);
        loop {
            (old, new) = match (old, new) {
                ([], []) => return,
                // Satzzeichen des Diktats: Die setzt das Modell.
                ([first, old_rest @ ..], _) if first.is_punctuation() => (old_rest, new),
                (_, [candidate, new_rest @ ..]) if candidate.is_punctuation() => {
                    self.punctuation(candidate.raw);
                    (old, new_rest)
                }
                ([], [candidate, new_rest @ ..]) => {
                    self.drop_insertion(candidate);
                    (old, new_rest)
                }
                ([first, old_rest @ ..], []) => {
                    self.keep(std::slice::from_ref(first));
                    (old_rest, new)
                }
                ([first, old_rest @ ..], [candidate, new_rest @ ..]) => self
                    .respaced(old, new)
                    .or_else(|| {
                        self.corrected(first, candidate)
                            .then_some((old_rest, new_rest))
                    })
                    .or_else(|| self.split(first, new).map(|after| (old_rest, after)))
                    .or_else(|| self.joined(old, candidate).map(|after| (after, new_rest)))
                    .unwrap_or_else(|| self.inserted_or_dropped(old, new)),
            };
        }
    }

    /// Passt das erste Wort des Diktats zu nichts: Kommt es in der Antwort
    /// später, hat das Modell davor etwas eingefügt (verworfen), sonst hat
    /// es das Wort gestrichen oder ausgetauscht (das Diktat bleibt).
    fn inserted_or_dropped<'t>(
        &mut self,
        old: &'t [Token<'a>],
        new: &'t [Token<'a>],
    ) -> (&'t [Token<'a>], &'t [Token<'a>]) {
        let ([first, old_rest @ ..], [candidate, new_rest @ ..]) = (old, new) else {
            return (old, new);
        };
        if new_rest.iter().any(|later| close(&first.core, &later.core)) {
            self.drop_insertion(candidate);
            (old, new_rest)
        } else {
            self.keep(std::slice::from_ref(first));
            (old_rest, new)
        }
    }

    /// Nur anders zusammen- oder getrennt geschrieben: Das erste Wort einer
    /// Seite ist genau die Folge mehrerer Wörter der anderen („i.V.m.“ ↔
    /// „i. V. m.“, „los gegangen“ ↔ „losgegangen“). Getrennt wird nur ein
    /// falsch geschriebenes Wort, zusammengeschrieben nur, wenn eins der
    /// Wörter falsch geschrieben ist; sonst bleibt das Diktat. Gibt zurück,
    /// was auf beiden Seiten danach offen ist.
    fn respaced<'t>(
        &mut self,
        old: &'t [Token<'a>],
        new: &'t [Token<'a>],
    ) -> Option<(&'t [Token<'a>], &'t [Token<'a>])> {
        let ([first, old_rest @ ..], [candidate, new_rest @ ..]) = (old, new) else {
            return None;
        };
        if let Some((parts, after)) = spelled_apart(&first.core, new) {
            if self.is_correct(&first.core) {
                // „Kaufvertragsabschluss“ war richtig.
                self.kept_correct = self.kept_correct.saturating_add(1);
                let together: String = parts.iter().map(|part| part.raw).collect();
                self.take(first, in_form_of(first.raw, &together));
            } else {
                self.take_parts(first, parts);
            }
            return Some((old_rest, after));
        }
        let (parts, after) = spelled_apart(&candidate.core, old)?;
        if self.may_join(parts) {
            self.take(first, candidate.raw);
        } else {
            self.keep(parts);
        }
        Some((after, new_rest))
    }

    /// Gleich oder ein kleiner Schreibfehler: die Fassung des Modells, außer
    /// das Wort darf nicht geändert werden. `false`, wenn die beiden nicht
    /// zueinander passen.
    fn corrected(&mut self, first: &Token<'a>, candidate: &Token<'a>) -> bool {
        if !close(&first.core, &candidate.core) {
            return false;
        }
        if first.core == candidate.core || self.may_change(&first.core, &candidate.core) {
            self.take(first, candidate.raw);
        } else {
            // „Hause“ → „Haus“: war schon richtig.
            self.kept_correct = self.kept_correct.saturating_add(1);
            self.take(first, in_form_of(first.raw, candidate.raw));
        }
        true
    }

    /// Ein falsch geschriebenes Wort, getrennt und dabei korrigiert:
    /// „losgegangn“ → „los gegangen“. Jeder Teil muss für sich zu einem
    /// Stück des Wortes passen, damit kein kurzes Wort dazukommt („geschaft“
    /// → „es geschafft“). Gibt zurück, was in der Antwort danach offen ist.
    fn split<'t>(&mut self, first: &Token<'a>, new: &'t [Token<'a>]) -> Option<&'t [Token<'a>]> {
        let [left, right, after @ ..] = new else {
            return None;
        };
        if self.is_correct(&first.core) || !splits_into(&first.core, &left.core, &right.core) {
            return None;
        }
        self.take_parts(first, std::slice::from_ref(left));
        self.emit(Piece {
            gap: "",
            word: Cow::Borrowed(right.raw),
            source: Source::Model {
                dictated: first.raw,
            },
        });
        Some(after)
    }

    /// Zwei Wörter, zusammengeschrieben und dabei korrigiert: „los
    /// gegangn“ → „losgegangen“. Jedes muss für sich zu einem Stück des
    /// neuen Wortes passen, damit keins verschwindet („no sabiamos“ →
    /// „sabíamos“). Gibt zurück, was im Diktat danach offen ist.
    fn joined<'t>(
        &mut self,
        old: &'t [Token<'a>],
        candidate: &Token<'a>,
    ) -> Option<&'t [Token<'a>]> {
        let ([left, right, after @ ..], Some(pair)) = (old, old.get(..2)) else {
            return None;
        };
        if !self.may_join(pair) || !splits_into(&candidate.core, &left.core, &right.core) {
            return None;
        }
        self.take(left, candidate.raw);
        Some(after)
    }

    /// Die Teile, in die das Modell das Wort `old` getrennt hat.
    fn take_parts(&mut self, old: &Token<'a>, parts: &[Token<'a>]) {
        for (index, part) in parts.iter().enumerate() {
            self.emit(Piece {
                gap: if index == 0 { old.gap } else { "" },
                word: Cow::Borrowed(part.raw),
                source: Source::Model { dictated: old.raw },
            });
        }
    }

    /// Darf das Modell `original` durch `polished` ersetzen? Nur, wenn
    /// dabei keine Verneinung kippt und `original` nicht richtig geschrieben
    /// ist oder beide leicht verwechselt werden.
    fn may_change(&self, original: &str, polished: &str) -> bool {
        !flips_negation(original, polished)
            && (!self.is_correct(original)
                || CONFUSABLE.iter().any(|&(a, b)| {
                    (a, b) == (original, polished) || (b, a) == (original, polished)
                }))
    }

    /// Ist das Wort (sein Kern) richtig geschrieben? Ungeprüft gilt jedes
    /// als richtig.
    fn is_correct(&self, core: &str) -> bool {
        match self.spelling {
            Spelling::Checked(correct) => correct.contains(core),
            Spelling::Unchecked => true,
        }
    }

    /// Darf das Modell diese Wörter zusammenschreiben? Nur, wenn eins davon
    /// falsch geschrieben ist: „zu sagen“ und „zusagen“ sind beide richtig,
    /// meinen aber anderes. Ungeprüft also nie.
    fn may_join(&self, parts: &[Token<'a>]) -> bool {
        parts.iter().any(|part| !self.is_correct(&part.core))
    }

    /// Wörter des Diktats unverändert übernehmen.
    fn keep(&mut self, old: &[Token<'a>]) {
        self.kept_originals = self.kept_originals.saturating_add(old.len());
        for token in old {
            self.emit(Piece {
                gap: token.gap,
                word: Cow::Borrowed(token.raw),
                source: Source::Dictation,
            });
        }
    }
}

/// Die Folge der ersten (mindestens zwei) `tokens`, deren Kerne
/// zusammen genau `whole` ergeben, und was danach kommt.
fn spelled_apart<'t, 'a>(
    whole: &str,
    tokens: &'t [Token<'a>],
) -> Option<(&'t [Token<'a>], &'t [Token<'a>])> {
    let mut rest = whole;
    for (index, token) in tokens.iter().enumerate() {
        if token.core.is_empty() {
            return None;
        }
        rest = rest.strip_prefix(token.core.as_str())?;
        if rest.is_empty() {
            return (index > 0).then(|| tokens.split_at(index + 1));
        }
    }
    None
}

/// Lässt sich `whole` so in zwei Stücke teilen, dass das erste zu `left`
/// und das zweite zu `right` passt ([`close`])?
fn splits_into(whole: &str, left: &str, right: &str) -> bool {
    whole.char_indices().skip(1).any(|(at, _)| {
        let (head, tail) = whole.split_at(at);
        close(head, left) && close(tail, right)
    })
}

/// Hat das Modell das erste Wort eines Satzes gestrichen und das nächste
/// großgeschrieben („no sabiamos“ → „Sabíamos“), steht jetzt das behaltene
/// Wort am Satzanfang: Es wird groß, das nächste wieder klein wie im
/// Diktat.
fn move_sentence_start(pieces: &mut [Piece<'_>]) {
    let mut at_start = true;
    let mut pieces = pieces.iter_mut().peekable();
    while let Some(piece) = pieces.next() {
        if at_start
            && let Some(next) = pieces.peek_mut()
            && moved_capital(piece, next)
        {
            piece.word = Cow::Owned(with_first_letter(&piece.word, LetterCase::Upper));
            next.word = Cow::Owned(with_first_letter(&next.word, LetterCase::Lower));
        }
        at_start = match piece.source {
            Source::Punctuation => at_start || ends_sentence(&piece.word),
            Source::Dictation | Source::Model { dictated: _ } => ends_sentence(&piece.word),
        };
    }
}

/// Steht `kept` (aus dem Diktat, klein) vor `next`, das das Modell gegen
/// das Diktat großgeschrieben hat?
fn moved_capital(kept: &Piece<'_>, next: &Piece<'_>) -> bool {
    let (Source::Dictation, Source::Model { dictated }) = (kept.source, next.source) else {
        return false;
    };
    first_letter_is(&kept.word, LetterCase::Lower)
        && first_letter_is(&next.word, LetterCase::Upper)
        && first_letter_is(dictated, LetterCase::Lower)
}

/// Groß- oder Kleinbuchstabe.
#[derive(Debug, Clone, Copy)]
enum LetterCase {
    Upper,
    Lower,
}

/// Ist der erste Buchstabe von `word` groß bzw. klein?
fn first_letter_is(word: &str, case: LetterCase) -> bool {
    word.chars()
        .find(|c| c.is_alphabetic())
        .is_some_and(|letter| match case {
            LetterCase::Upper => letter.is_uppercase(),
            LetterCase::Lower => letter.is_lowercase(),
        })
}

/// `word` mit dem ersten Buchstaben groß bzw. klein.
fn with_first_letter(word: &str, case: LetterCase) -> String {
    let mut done = false;
    word.chars()
        .flat_map(|c| {
            let change = !done && c.is_alphabetic();
            done |= change;
            let changed: Vec<char> = match (change, case) {
                (false, LetterCase::Upper | LetterCase::Lower) => vec![c],
                (true, LetterCase::Upper) => c.to_uppercase().collect(),
                (true, LetterCase::Lower) => c.to_lowercase().collect(),
            };
            changed
        })
        .collect()
}

/// Endet mit `word` ein Satz (auch vor schließenden Anführungszeichen oder
/// Klammern)?
fn ends_sentence(word: &str) -> bool {
    word.trim_end_matches(['"', '“', '”', '«', '»', '‘', '’', '\'', ')', ']'])
        .ends_with(['.', '!', '?'])
}

/// Das Wort `original` mit Satzzeichen und Großschreibung am Anfang aus
/// `polished`: „hause“ + „Haus,“ → „Hause,“.
fn in_form_of(original: &str, polished: &str) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '§';
    let word = original.trim_matches(|c: char| !is_word(c));
    let before = polished
        .find(is_word)
        .and_then(|start| polished.get(..start))
        .unwrap_or_default();
    let after = polished
        .rfind(is_word)
        .and_then(|end| polished.get(end..))
        .map(|tail| tail.trim_start_matches(is_word))
        .unwrap_or_default();
    let mut letters = word.chars();
    let first = letters.next().map(|first| {
        if polished
            .chars()
            .find(|c| is_word(*c))
            .is_some_and(char::is_uppercase)
        {
            first.to_uppercase().collect::<String>()
        } else {
            first.to_string()
        }
    });
    format!(
        "{before}{}{}{after}",
        first.unwrap_or_default(),
        letters.as_str()
    )
}

/// Zerlegt Text ohne Leerraum am Anfang in Wörter samt Satzzeichen und dem
/// Leerraum davor.
fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let (mut gap, mut rest) = ("", text);
    while !rest.is_empty() {
        // Ohne Leerraum danach reicht das Wort bis zum Ende.
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (raw, after) = rest.split_at(end);
        tokens.push(Token {
            gap,
            raw,
            core: token_core(raw),
        });
        (gap, rest) = split_off_whitespace(after);
    }
    tokens
}

/// Teilt `text` in den Leerraum am Anfang und den Rest.
fn split_off_whitespace(text: &str) -> (&str, &str) {
    text.split_at(text.len() - text.trim_start().len())
}

/// Vergleichskern eines Tokens: der [`core()`] eines Wortes; hat es keinen,
/// die Zeichen außer Satzzeichen („&“, „%“), damit „&“ nicht „+“ gleicht.
/// Leer nur für reine Satzzeichen.
fn token_core(raw: &str) -> String {
    let word = core(raw);
    if !word.is_empty() {
        return word;
    }
    raw.chars().filter(|c| !PUNCTUATION.contains(*c)).collect()
}

/// Vergleichskern eines Wortes: klein, ohne Satzzeichen. Punkt und Komma
/// zwischen zwei Ziffern bleiben, damit „2,5“ nicht „25“ gleicht.
#[must_use]
pub(crate) fn core(raw: &str) -> String {
    let previous = std::iter::once(None).chain(raw.chars().map(Some));
    let next = raw.chars().skip(1).map(Some).chain(std::iter::once(None));
    let is_digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
    raw.chars()
        .zip(previous.zip(next))
        .filter(|&(c, (before, after))| {
            c.is_alphanumeric()
                || c == '§'
                || (matches!(c, '.' | ',') && is_digit(before) && is_digit(after))
        })
        .flat_map(|(c, _)| c.to_lowercase())
        .collect()
}

/// Gleich oder nur ein kleiner Schreibfehler auseinander? Erlaubt sind 1
/// Zeichen bei Wörtern bis 4 Zeichen, 2 bis 8, sonst 3. Zahlen müssen
/// gleich sein: „15“ und „16“ sind kein Schreibfehler, ebenso andere
/// Zeichen als Buchstaben („&“ und „+“). Ist eins mindestens
/// doppelt so lang wie das andere, passen sie nicht: „z“ ist nicht „zb“.
fn close(original: &str, polished: &str) -> bool {
    if original == polished {
        return true;
    }
    if [original, polished]
        .iter()
        .any(|word| word.chars().any(|c| !c.is_alphabetic()))
    {
        return false;
    }
    let (original_length, polished_length) = (original.chars().count(), polished.chars().count());
    if original_length.min(polished_length) * 2 <= original_length.max(polished_length) {
        return false;
    }
    let length = original_length.max(polished_length);
    let allowed = match length {
        0..=4 => 1,
        5..=8 => 2,
        _ => 3,
    };
    strsim::levenshtein(original, polished) <= allowed
}

/// Vorsilben, die ein Wort verneinen: „unwirksam“, „inkorrekt“,
/// „immobil“, „illegal“, „irrelevant“, „dishonest“, „nonverbal“ und das
/// „k“ von „kein“.
const NEGATING_PREFIXES: [&str; 8] = ["un", "in", "im", "il", "ir", "dis", "non", "k"];

/// Macht die Änderung aus einem bejahten Wort ein verneintes oder
/// umgekehrt? „wirksam“ ↔ „unwirksam“, „ein“ ↔ „kein“ (Kerne), auch wenn
/// eins von beiden falsch geschrieben ist („keinn“ → „ein“). Tragen beide
/// die Vorsilbe („kein“ → „keine“), kippt nichts.
fn flips_negation(original: &str, polished: &str) -> bool {
    let negates = |negated: &str, plain: &str| {
        NEGATING_PREFIXES.iter().any(|prefix| {
            !plain.starts_with(prefix)
                && negated
                    .strip_prefix(prefix)
                    .is_some_and(|rest| close(rest, plain))
        })
    };
    negates(original, polished) || negates(polished, original)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Richtig geschriebene Wörter, wie die Rechtschreibprüfung sie meldet.
    fn known(words: &[&str]) -> BTreeSet<String> {
        words.iter().map(|word| core(word)).collect()
    }

    /// Geprüft, aber kein Wort richtig geschrieben: Das Modell darf alles
    /// korrigieren, was nach Schreibfehler aussieht.
    fn guard(original: &str, polished: &str) -> String {
        keep_corrections(original, polished, Spelling::Checked(&BTreeSet::new())).text
    }

    #[test]
    fn takes_spelling_case_and_punctuation() {
        assert_eq!(
            guard(
                "ich gehe heute nach hause und dan esse ich ein spiegelei",
                "Ich gehe heute nach Hause, und dann esse ich ein Spiegelei."
            ),
            "Ich gehe heute nach Hause, und dann esse ich ein Spiegelei."
        );
        assert_eq!(
            guard("ich glaube das wir kommen", "Ich glaube, dass wir kommen."),
            "Ich glaube, dass wir kommen."
        );
    }

    #[test]
    fn takes_joined_and_split_words() {
        assert_eq!(
            guard("dann bin ich los gegangen", "Dann bin ich losgegangen."),
            "Dann bin ich losgegangen."
        );
        assert_eq!(
            guard("ein Kaufvertragsabschluss", "ein Kaufvertrags abschluss"),
            "ein Kaufvertrags abschluss"
        );
    }

    #[test]
    fn rejects_inserted_and_replaced_words() {
        let result = keep_corrections(
            "das gilt auch wenn er die sache nicht benutzt hat",
            "Das gilt auch, auch wenn er die Sache noch nicht benutzt hat.",
            Spelling::Checked(&BTreeSet::new()),
        );
        assert_eq!(
            result.text,
            "Das gilt auch, wenn er die Sache nicht benutzt hat."
        );
        assert_eq!(result.dropped_insertions, 2);
        assert_eq!(
            guard(
                "erst habe ich gegessen weil ich hunger hatte",
                "Erst habe ich gegessen, weil ich hungrig war."
            ),
            "Erst habe ich gegessen, weil ich hunger hatte."
        );
    }

    #[test]
    fn keeps_words_the_model_dropped() {
        let result = keep_corrections(
            "Der Käufer muss nach § 433 II BGB zahlen",
            "Der Käufer muss zahlen.",
            Spelling::Checked(&BTreeSet::new()),
        );
        assert_eq!(result.text, "Der Käufer muss nach § 433 II BGB zahlen.");
        assert_eq!(result.kept_originals, 5);
    }

    #[test]
    fn keeps_correctly_spelled_words() {
        let correct = known(&[
            "ich", "gehe", "heute", "nach", "Hause", "das", "wir", "kommen",
        ]);
        let result = keep_corrections(
            "ich gehe heute nach Hause dan kommen wir",
            "Ich gehe heute nach Haus, dann kommen wir.",
            Spelling::Checked(&correct),
        );
        assert_eq!(result.text, "Ich gehe heute nach Hause, dann kommen wir.");
        assert_eq!(result.kept_correct, 1);
        // Verwechslungen darf das Modell korrigieren.
        assert_eq!(
            keep_corrections(
                "ich glaube das wir kommen",
                "Ich glaube, dass wir kommen.",
                Spelling::Checked(&correct)
            )
            .text,
            "Ich glaube, dass wir kommen."
        );
        // Ein richtig geschriebenes Wort wird nicht auseinandergerissen.
        assert_eq!(
            keep_corrections(
                "ein Kaufvertragsabschluss",
                "ein Kaufvertrags abschluss",
                Spelling::Checked(&known(&["ein", "Kaufvertragsabschluss"]))
            )
            .text,
            "ein Kaufvertragsabschluss"
        );
    }

    #[test]
    fn spaced_abbreviation_keeps_all_its_parts() {
        // „i.V.m.“ kennt die Rechtschreibprüfung nicht; das Modell setzt die
        // Leerzeichen nach Duden.
        let correct = known(&["das", "gilt", "BGB", "hier"]);
        let checked = |original: &str, polished: &str| {
            keep_corrections(original, polished, Spelling::Checked(&correct)).text
        };
        assert_eq!(
            checked("das gilt i.V.m. § 280 BGB", "Das gilt i. V. m. § 280 BGB."),
            "Das gilt i. V. m. § 280 BGB."
        );
        assert_eq!(
            checked("das gilt z.B. hier", "Das gilt z. B. hier."),
            "Das gilt z. B. hier."
        );
        // Andersherum und ungeprüft: Das Diktat bleibt, ganz.
        assert_eq!(
            keep_corrections(
                "das gilt z. B. hier",
                "Das gilt z.B. hier.",
                Spelling::Unchecked
            )
            .text,
            "Das gilt z. B. hier."
        );
        assert_eq!(
            checked("das gilt z. B. hier", "Das gilt z.B. hier."),
            "Das gilt z.B. hier."
        );
        // Kennt die Prüfung die Abkürzung, bleibt sie, wie sie war.
        assert_eq!(
            keep_corrections(
                "das gilt i.V.m. § 280 BGB",
                "Das gilt i. V. m. § 280 BGB.",
                Spelling::Checked(&known(&["das", "gilt", "i.V.m", "BGB"]))
            )
            .text,
            "Das gilt i.V.m. § 280 BGB."
        );
    }

    #[test]
    fn split_word_keeps_its_last_part() {
        // Der erste Teil allein ist kaum anders als das ganze Wort.
        assert_eq!(
            guard("der bundesgerichtshof", "Der Bundesgerichts hof."),
            "Der Bundesgerichts hof."
        );
        assert_eq!(
            keep_corrections(
                "der bundesgerichtshof",
                "Der Bundesgerichts hof.",
                Spelling::Checked(&known(&["der", "bundesgerichtshof"]))
            )
            .text,
            "Der Bundesgerichtshof."
        );
    }

    #[test]
    fn join_does_not_drop_a_short_word() {
        // Spanisch „no“: das Modell streicht es und setzt den Akzent.
        assert_eq!(
            keep_corrections(
                "no sabiamos nada",
                "Sabíamos nada.",
                Spelling::Checked(&known(&["no", "nada"]))
            )
            .text,
            "No sabíamos nada."
        );
        assert_eq!(
            keep_corrections(
                "wir haben es geschaft",
                "Wir haben geschafft.",
                Spelling::Checked(&known(&["wir", "haben", "es"]))
            )
            .text,
            "Wir haben es geschafft."
        );
        // Beide richtig geschrieben: nicht zusammen, auch geprüft.
        assert_eq!(
            keep_corrections(
                "ich muss es zu sagen",
                "Ich muss es zusagen.",
                Spelling::Checked(&known(&["ich", "muss", "es", "zu", "sagen"]))
            )
            .text,
            "Ich muss es zu sagen."
        );
        // Zusammen und dabei korrigiert geht weiter.
        assert_eq!(
            guard("dann bin ich los gegangn", "Dann bin ich losgegangen."),
            "Dann bin ich losgegangen."
        );
    }

    #[test]
    fn split_does_not_insert_a_short_word() {
        assert_eq!(
            keep_corrections(
                "ich habe geschaft",
                "Ich habe es geschafft.",
                Spelling::Checked(&known(&["ich", "habe"]))
            )
            .text,
            "Ich habe geschafft."
        );
        // Getrennt und dabei korrigiert geht weiter.
        assert_eq!(
            guard("dann bin ich losgegangn", "Dann bin ich los gegangen."),
            "Dann bin ich los gegangen."
        );
    }

    #[test]
    fn dropped_first_word_takes_the_capital() {
        assert_eq!(guard("aber wir komen", "Wir kommen."), "Aber wir kommen.");
        // Mitten im Satz bleibt die Großschreibung des Modells.
        assert_eq!(
            guard("ich habe das auto", "Ich habe Auto."),
            "Ich habe das Auto."
        );
    }

    #[test]
    fn replaced_dash_is_not_doubled() {
        let correct = known(&["das", "ist", "wie", "gesagt", "gut"]);
        assert_eq!(
            keep_corrections(
                "das ist – wie gesagt – gut",
                "Das ist, wie gesagt, gut.",
                Spelling::Checked(&correct)
            )
            .text,
            "Das ist, wie gesagt, gut."
        );
        // Gedankenstriche des Modells um behaltene Wörter bleiben.
        assert_eq!(
            guard("das ist wie gesagt gut", "Das ist – wie gesagt – gut."),
            "Das ist – wie gesagt – gut."
        );
        // Französisch: Das Modell rückt das Fragezeichen an oder ab.
        assert_eq!(guard("c'est ? oui", "C'est? Oui."), "C'est? Oui.");
        assert_eq!(
            guard("tu viens? je pars", "Tu viens ? Je pars."),
            "Tu viens ? Je pars."
        );
    }

    #[test]
    fn dropped_insertion_takes_its_punctuation_along() {
        assert_eq!(guard("das ist gut", "Das ist gut, oder?"), "Das ist gut?");
        assert_eq!(
            guard("ich komme morgen", "Ich komme, wenn es geht, morgen."),
            "Ich komme morgen."
        );
        assert_eq!(
            guard("ich komme morgen", "Ich komme – wenn es geht – morgen."),
            "Ich komme morgen."
        );
        assert_eq!(
            guard(
                "ich komme wir sehen uns",
                "Ich komme, bis dann. Wir sehen uns."
            ),
            "Ich komme. Wir sehen uns."
        );
    }

    #[test]
    fn symbols_count_as_words() {
        assert_eq!(guard("a & b", "A + b."), "A & b.");
        assert_eq!(
            guard("müller & söhne", "Müller und Söhne."),
            "Müller & Söhne."
        );
    }

    #[test]
    fn protected_word_takes_punctuation_and_capital() {
        assert_eq!(in_form_of("hause", "Haus,"), "Hause,");
        assert_eq!(in_form_of("was", "„etwas“."), "„was“.");
        assert_eq!(in_form_of("Hause.", "haus"), "Hause");
    }

    #[test]
    fn ignores_a_translation() {
        assert_eq!(
            guard(
                "i think we should meet tomorrow",
                "Ich denke, wir sollten uns morgen treffen."
            ),
            "i think we should meet tomorrow."
        );
    }

    #[test]
    fn never_changes_numbers() {
        assert_eq!(
            guard("laut § 433 II BGB", "Laut § 434 II BGB."),
            "Laut § 433 II BGB."
        );
        assert_eq!(
            guard("es kostet 15 euro", "Es kostet 16 Euro."),
            "Es kostet 15 Euro."
        );
        assert_eq!(
            guard("es sind 10.000 euro", "Es sind 100.000 Euro."),
            "Es sind 10.000 Euro."
        );
        assert_eq!(
            guard("es sind 2,5 prozent", "Es sind 25 Prozent."),
            "Es sind 2,5 Prozent."
        );
        // Satzzeichen an einer Zahl darf das Modell setzen.
        assert_eq!(
            guard("im jahr 2026 also", "Im Jahr 2026, also"),
            "Im Jahr 2026, also"
        );
    }

    #[test]
    fn never_flips_a_negation() {
        assert_eq!(
            guard("das ist unwirksamm", "Das ist wirksam."),
            "Das ist unwirksamm."
        );
        assert_eq!(
            guard("das ist wirksahm", "Das ist unwirksam."),
            "Das ist wirksahm."
        );
        assert_eq!(
            guard("er hat ein auto", "Er hat kein Auto."),
            "Er hat ein Auto."
        );
        assert_eq!(
            guard("er hat keinn auto", "Er hat kein Auto."),
            "Er hat kein Auto."
        );
    }

    #[test]
    fn misspelled_negation_does_not_flip() {
        assert_eq!(
            guard("er hat keinn auto", "Er hat ein Auto."),
            "Er hat keinn Auto."
        );
        assert_eq!(
            guard("er hat einn auto", "Er hat kein Auto."),
            "Er hat einn Auto."
        );
        assert_eq!(
            guard("er hat keinen hund", "Er hat einen Hund."),
            "Er hat keinen Hund."
        );
        // Jede Vorsilbe, die verneint: falsch geschrieben, vom Modell
        // „korrigiert“ und dabei bejaht – und umgekehrt.
        for (negated, plain) in [
            ("inkorrektt", "korrekt"),
            ("immateriel", "materiell"),
            ("illegitimm", "legitim"),
            ("irrelevantt", "relevant"),
            ("dishonest", "honest"),
            ("nonverbal", "verbal"),
        ] {
            assert_eq!(guard(negated, plain), negated);
        }
        for (plain, negated) in [
            ("korrekt", "inkorrekt"),
            ("mobil", "immobil"),
            ("legal", "illegal"),
            ("relevant", "irrelevant"),
            ("honest", "dishonest"),
            ("verbal", "nonverbal"),
        ] {
            assert_eq!(guard(plain, negated), plain);
        }
    }

    #[test]
    fn negation_check_allows_endings() {
        // Gleiche Vorsilbe auf beiden Seiten: nichts kippt.
        assert_eq!(
            guard("er hat kein ahnung", "Er hat keine Ahnung."),
            "Er hat keine Ahnung."
        );
        assert_eq!(
            guard("das ist unwirksam", "Das ist unwirksame."),
            "Das ist unwirksame."
        );
    }

    #[test]
    fn unchecked_allows_only_case_punctuation_and_confusables() {
        let unchecked = |original: &str, polished: &str| {
            keep_corrections(original, polished, Spelling::Unchecked).text
        };
        assert_eq!(
            unchecked("das ist unwirksam", "Das ist wirksam."),
            "Das ist unwirksam."
        );
        assert_eq!(
            unchecked("ich muss es zu sagen", "Ich muss es zusagen."),
            "Ich muss es zu sagen."
        );
        assert_eq!(
            unchecked("ich glaube das wir kommen", "Ich glaube, dass wir kommen."),
            "Ich glaube, dass wir kommen."
        );
    }

    #[test]
    fn keeps_line_breaks_from_voice_commands() {
        // „Das war gut. Neuer Absatz. Morgen geht es weiter.“
        let paragraphs = "Das war gut.\n\nMorgen geht es weiter.";
        assert_eq!(
            keep_corrections(paragraphs, paragraphs, Spelling::Unchecked).text,
            paragraphs
        );
        // Das Modell lässt die Umbrüche weg: Sie kommen aus dem Diktat.
        assert_eq!(
            guard(
                "\nerstens\nzweitens\n\ndas war gut \n",
                "Erstens, zweitens. Das war gut."
            ),
            "\nErstens,\nzweitens.\n\nDas war gut. \n"
        );
    }

    #[test]
    fn core_keeps_separators_inside_numbers() {
        assert_eq!(core("2,5"), "2,5");
        assert_eq!(core("10.000,"), "10.000");
        assert_eq!(core("„Haus.“"), "haus");
        assert_eq!(core("§433."), "§433");
    }

    /// Erzeugt Texte aus Wörtern, Abkürzungen, Zahlen, Satzzeichen und
    /// Zeichen, getrennt von Leerzeichen, Umbrüchen und Leerzeilen, auch am
    /// Anfang und am Ende.
    mod generate {
        use proptest::prelude::*;

        const WORDS: [&str; 44] = [
            "das",
            "dass",
            "Das",
            "kein",
            "keinn",
            "ein",
            "unwirksam",
            "wirksam",
            "i.V.m.",
            "i.",
            "V.",
            "m.",
            "z.B.",
            "z.",
            "B.",
            "los",
            "gegangen",
            "losgegangen",
            "es",
            "geschaft",
            "geschafft",
            "no",
            "sabíamos",
            "Haus",
            "Hause,",
            "§",
            "433",
            "2,5",
            "–",
            "-",
            "?",
            "„Zitat“",
            "(so)",
            "&",
            "%",
            "oder?",
            "gut,",
            "Müller",
            "x",
            "ab",
            "a",
            "dann.",
            "zu",
            "sagen",
        ];
        const GAPS: [&str; 6] = [" ", "  ", "\n", "\n\n", "\t", " \u{a0}"];

        fn gap() -> impl Strategy<Value = &'static str> {
            prop::sample::select(&GAPS[..])
        }

        fn word() -> impl Strategy<Value = &'static str> {
            prop::sample::select(&WORDS[..])
        }

        pub(super) fn text() -> impl Strategy<Value = String> {
            (
                prop::option::of(gap()),
                prop::collection::vec((word(), gap()), 0..12),
                prop::option::of(gap()),
            )
                .prop_map(|(lead, words, trail)| assemble(lead, &words, trail))
        }

        /// Die Wörter mit dem Leerraum nach ihnen, ohne den nach dem letzten.
        fn assemble(lead: Option<&str>, words: &[(&str, &str)], trail: Option<&str>) -> String {
            let mut text = String::from(lead.unwrap_or_default());
            let mut separator = "";
            for &(word, gap) in words {
                text.push_str(separator);
                text.push_str(word);
                separator = gap;
            }
            text.push_str(trail.unwrap_or_default());
            text
        }

        /// Beliebige Zeichen, auch Leerraum aller Art.
        pub(super) fn chars() -> impl Strategy<Value = String> {
            prop::collection::vec(any::<char>(), 0..40)
                .prop_map(|chars| chars.into_iter().collect())
        }
    }

    /// Die Kerne der Wörter (ohne reine Satzzeichen) eines Textes.
    fn word_cores(text: &str) -> Vec<String> {
        tokens(text.trim_start())
            .into_iter()
            .filter(|token| !token.is_punctuation())
            .map(|token| token.core)
            .collect()
    }

    /// Gleich oder ein Paar aus [`CONFUSABLE`].
    fn same_or_confusable(original: &str, guarded: &str) -> bool {
        original == guarded
            || CONFUSABLE
                .iter()
                .any(|&(a, b)| (a, b) == (original, guarded) || (b, a) == (original, guarded))
    }

    proptest::proptest! {
        /// Gibt das Modell den Text unverändert zurück, kommt er genau so
        /// heraus, Leerraum eingeschlossen.
        #[test]
        fn unchanged_text_stays_unchanged(
            text in proptest::prop_oneof![generate::text(), generate::chars()],
        ) {
            let all = word_cores(&text).into_iter().collect::<BTreeSet<_>>();
            let none = BTreeSet::new();
            for spelling in [Spelling::Unchecked, Spelling::Checked(&none), Spelling::Checked(&all)] {
                proptest::prop_assert_eq!(&keep_corrections(&text, &text, spelling).text, &text);
            }
        }

        /// Sind alle Wörter richtig geschrieben (oder ungeprüft), darf das
        /// Modell weder trennen noch zusammenschreiben, einfügen noch
        /// streichen: Es bleiben dieselben Wörter, nur Verwechslungen
        /// („das“ → „dass“) darf es tauschen.
        #[test]
        fn correct_words_keep_their_number(
            original in generate::text(),
            polished in generate::text(),
        ) {
            let before = word_cores(&original);
            let correct = before.iter().cloned().collect::<BTreeSet<_>>();
            for spelling in [Spelling::Unchecked, Spelling::Checked(&correct)] {
                let after = word_cores(&keep_corrections(&original, &polished, spelling).text);
                proptest::prop_assert_eq!(before.len(), after.len(), "{:?}", after);
                for (dictated, guarded) in before.iter().zip(&after) {
                    proptest::prop_assert!(
                        same_or_confusable(dictated, guarded),
                        "{} → {}", dictated, guarded
                    );
                }
            }
        }
    }
}
