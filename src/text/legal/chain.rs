//! Ein Zitat im Aufbau und wie es geschrieben wird: jede Norm mit ihrem
//! Zeichen, ihren Teilen und ihrem Gesetz.

use super::super::numbers;
use super::super::statutes::{Statute, Unit};
use super::super::words::Word;
use super::{
    Abbreviations, CitationStyle, Head, Item, MAX_SUBDIVISION, Number, Part, PartKind, Subdivision,
    WrittenSentence,
};

/// Ein Zitat im Aufbau: die erste Norm und jede nach „i.V.m.“, jede mit
/// eigenem Zeichen („§ 823 I BGB i.V.m. Art. 2 I GG“).
pub(super) struct Chain {
    first: Segment,
    /// Die Normen nach „i.V.m.“.
    joined: Vec<Segment>,
    /// Ob eine bloße Zahl am Anfang schon als Norm ausgewiesen ist.
    evidence: Evidence,
}

/// Ist eine bloße Zahl am Zitatanfang schon als Norm ausgewiesen?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evidence {
    /// Noch nicht: Es fehlt ein Normteil oder Gesetz.
    Pending,
    /// Ein Normteil, ein Gesetz oder „i.V.m.“ ist gefolgt.
    Confirmed,
}

/// Eine Norm in einem Zitat: ihr Kopf, ihre Nummern und Teile, ihr Gesetz.
struct Segment {
    head: Head,
    /// „i.V.m.“ vor dieser Norm; die erste hat keins.
    joint: Option<Word>,
    /// Satzzeichen vor dem Zeichen („(“).
    lead: String,
    /// Leerraum nach dem Zeichen.
    head_space: String,
    /// Alles nach dem Zeichen, schon umgeformt.
    words: Vec<Word>,
    /// Eine oder mehrere Nummern („§“ oder „§§“).
    count: Count,
    /// Zählweise des eigenen Gesetzes.
    unit: Option<Unit>,
    /// Was zuletzt kam.
    last: Last,
}

/// Wie viele Nummern eine Norm nennt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Count {
    /// „§ 823“
    One,
    /// „§§ 177 und 179“, „§§ 3-5“
    Several,
}

/// Das letzte Glied einer Norm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Last {
    /// Die Paragrafen- oder Artikelnummer.
    Norm,
    /// Ein Normteil („Absatz 2“).
    Part(Part),
    /// „analog“, „ff.“
    Modifier,
    /// Das Gesetz; danach darf nur noch „i.V.m.“ folgen.
    Statute,
}

impl Last {
    /// Kam zuletzt ein Absatz?
    const fn after_subsection(self) -> bool {
        match self {
            Self::Part(part) => part.is_subsection(),
            Self::Norm | Self::Modifier | Self::Statute => false,
        }
    }

    /// Die Art des Normteils, wenn zuletzt einer kam.
    const fn part_kind(self) -> Option<PartKind> {
        match self {
            Self::Part(part) => Some(part.kind()),
            Self::Norm | Self::Modifier | Self::Statute => None,
        }
    }
}

impl Segment {
    /// Eine Norm aus Kopf (`head_word`: das Kopf-Wort oder bei einer bloßen
    /// Zahl die Zahl selbst) und Nummer.
    fn new(head: Head, head_word: &Word, number_word: &Word, number: &Number) -> Self {
        let (head_space, number_lead) = match head {
            Head::Bare => (" ".to_owned(), String::new()),
            Head::Paragraph | Head::Paragraphs | Head::Article => {
                (head_word.space.clone(), number_word.lead.clone())
            }
        };
        let first = Word {
            lead: number_lead,
            ..rendered(&number.text, number_word)
        };
        let count = match head {
            Head::Paragraphs => Count::Several,
            Head::Bare | Head::Paragraph | Head::Article => number.count(),
        };
        Self {
            head,
            joint: None,
            lead: head_word.lead.clone(),
            head_space,
            words: vec![first],
            count,
            unit: None,
            last: Last::Norm,
        }
    }

    /// Die Wörter der Norm: „i.V.m.“, das Zeichen (wenn sie eins hat) und
    /// der Rest. `unit` ist die Zählweise ihres Gesetzes oder, ohne eigenes,
    /// die des nächsten im Zitat („Art. 2 I i.V.m. 1 I GG“).
    fn render(self, position: Position, unit: Option<Unit>) -> Vec<Word> {
        let signed = match position {
            Position::First => true,
            Position::Joined => match self.head {
                Head::Bare => false,
                Head::Paragraph | Head::Paragraphs | Head::Article => true,
            },
        };
        let sign = signed.then(|| Word {
            lead: self.lead,
            space: self.head_space,
            ..sign(self.head, self.count, unit)
        });
        self.joint
            .into_iter()
            .chain(sign)
            .chain(self.words)
            .collect()
    }
}

/// Wo eine Norm im Zitat steht.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    /// Am Anfang: Eine bloße Zahl bekommt ihr Zeichen vom Gesetz.
    First,
    /// Nach „i.V.m.“: Eine bloße Zahl bleibt ohne Zeichen („i.V.m. 1 I GG“).
    Joined,
}

impl Chain {
    /// Neues Zitat aus Kopf („§“ oder die Zahl selbst) und Nummer.
    pub(super) fn new(head: Head, head_word: &Word, number_word: &Word, number: &Number) -> Self {
        Self {
            first: Segment::new(head, head_word, number_word, number),
            joined: Vec::new(),
            evidence: Evidence::Pending,
        }
    }

    /// Die Norm, an der gerade gebaut wird.
    fn current(&self) -> &Segment {
        match self.joined.last() {
            Some(segment) => segment,
            None => &self.first,
        }
    }

    /// Die Norm, an der gerade gebaut wird, zum Ändern.
    fn current_mut(&mut self) -> &mut Segment {
        match self.joined.last_mut() {
            Some(segment) => segment,
            None => &mut self.first,
        }
    }

    /// Kann ein Absatz folgen, weil als Letztes die Nummer der Norm oder
    /// „analog“/„ff.“ kam?
    pub(super) fn takes_a_subsection(&self) -> bool {
        match self.current().last {
            Last::Norm | Last::Modifier => true,
            Last::Part(_) | Last::Statute => false,
        }
    }

    /// Kam als Letztes ein Gesetz? Dann darf nur noch „i.V.m.“ folgen.
    pub(super) fn ends_with_statute(&self) -> bool {
        match self.current().last {
            Last::Statute => true,
            Last::Norm | Last::Part(_) | Last::Modifier => false,
        }
    }

    /// Wie ein Gesetzeskürzel hier geschrieben sein muss: Solange eine
    /// bloße Zahl nicht als Norm bestätigt ist, wie üblich.
    pub(super) const fn abbreviations(&self) -> Abbreviations {
        match self.first.head {
            Head::Bare => match self.evidence {
                Evidence::Pending => Abbreviations::AsUsual,
                Evidence::Confirmed => Abbreviations::AnyCase,
            },
            Head::Paragraph | Head::Paragraphs | Head::Article => Abbreviations::AnyCase,
        }
    }

    /// Endet das letzte Glied mit einem Komma?
    pub(super) fn ends_with_comma(&self) -> bool {
        self.current()
            .words
            .last()
            .is_some_and(|word| word.trail.ends_with(','))
    }

    /// Die Stufe, auf der eine Zahl nach „und“ noch zum Teil davor gehört
    /// („Absatz 2 und 3“): eine mit Nummer, und die Zahl ist größer, aber
    /// nicht über [`MAX_SUBDIVISION`].
    pub(super) fn same_level(&self, value: u32) -> Option<Subdivision> {
        match self.current().last {
            Last::Part(Part::Numbered { level, value: last })
                if value <= MAX_SUBDIVISION && value > last =>
            {
                Some(level)
            }
            Last::Part(Part::Numbered { level: _, value: _ } | Part::Letter(_))
            | Last::Norm
            | Last::Modifier
            | Last::Statute => None,
        }
    }

    /// Hängt ein Glied an.
    pub(super) fn push(&mut self, item: Item<'_>, style: CitationStyle) {
        match item {
            Item::Part { part, first, last } => {
                if self.current().last.part_kind() == Some(part.kind()) {
                    self.separate();
                }
                self.push_part(part, (first, last), style);
            }
            Item::Written {
                roman,
                value,
                sentence,
            } => self.push_written(roman, value, sentence.as_ref(), style),
            Item::Connected {
                connector,
                part,
                first,
                last,
            } => {
                self.current_mut().words.push(connector.clone());
                self.push_part(part, (first, last), style);
            }
            Item::Same {
                connector,
                level,
                value,
                number,
            } => {
                let text = same_level_text(level, value, style);
                let segment = self.current_mut();
                segment.words.extend(connector.cloned());
                segment.words.push(rendered(&text, number));
                segment.last = Last::Part(Part::Numbered { level, value });
            }
            Item::Norm {
                connector,
                number,
                word,
            } => self.push_norm(connector, &number, word),
            Item::Modifier { text, word } => {
                let segment = self.current_mut();
                segment.words.push(abbreviation(text, word, word));
                segment.last = Last::Modifier;
            }
            Item::Statute {
                statute,
                first,
                last,
            } => self.push_statute(&statute, (first, last)),
            Item::Combined {
                first,
                last,
                head,
                number,
                word,
            } => self.push_combined((first, last), head, &number, word),
        }
    }

    /// Ausgeschriebene Gliederung wie „II 1“: Absatz in römischen Ziffern,
    /// dahinter vielleicht der Satz.
    fn push_written(
        &mut self,
        roman: &Word,
        value: u32,
        sentence: Option<&WrittenSentence<'_>>,
        style: CitationStyle,
    ) {
        let evidence = self.evidence;
        let subsection = Part::Numbered {
            level: Subdivision::Subsection,
            value,
        };
        self.push_part(subsection, (roman, roman), style);
        if let Some(WrittenSentence {
            word,
            value: number,
        }) = sentence
        {
            let part = Part::Numbered {
                level: Subdivision::Sentence,
                value: *number,
            };
            self.push_part(part, (word, word), style);
        }
        // So geschrieben weist es eine bloße Zahl nicht als Norm aus.
        self.evidence = evidence;
    }

    /// Eine weitere Norm derselben Kette („§§ 823, 826“).
    fn push_norm(&mut self, connector: Option<&Word>, number: &Number, word: &Word) {
        let segment = self.current_mut();
        segment.words.extend(connector.cloned());
        segment.words.push(rendered(&number.text, word));
        segment.count = Count::Several;
        segment.last = Last::Norm;
    }

    /// Das Gesetz am Ende der Kette; danach ist sie sicher ein Zitat.
    fn push_statute(&mut self, statute: &Statute, (first, last): (&Word, &Word)) {
        let segment = self.current_mut();
        segment.words.push(Word {
            lead: first.lead.clone(),
            ..rendered(&statute.name, last)
        });
        segment.unit = Some(statute.unit);
        segment.last = Last::Statute;
        self.evidence = Evidence::Confirmed;
    }

    /// „Absatz 2“ → „II“ bzw. „Abs. 2“.
    fn push_part(&mut self, part: Part, (first, last): (&Word, &Word), style: CitationStyle) {
        let segment = self.current_mut();
        let mut rendered_part = part_text(part, segment.last, style)
            .into_iter()
            .map(|(core, trail)| Word::new(&core, trail, " "))
            .collect::<Vec<_>>();
        if let Some(word) = rendered_part.first_mut() {
            word.lead.clone_from(&first.lead);
        }
        if let Some(word) = rendered_part.last_mut() {
            word.trail = format!("{}{}", word.trail, last.trail);
            word.space.clone_from(&last.space);
        }
        segment.words.extend(rendered_part);
        segment.last = Last::Part(part);
        self.evidence = Evidence::Confirmed;
    }

    /// „in Verbindung mit Artikel 1“ → „i.V.m. Art. 1“: eine neue Norm.
    fn push_combined(
        &mut self,
        (first, last): (&Word, &Word),
        head: Option<(Head, &Word)>,
        number: &Number,
        word: &Word,
    ) {
        let (head, head_word) =
            head.map_or((Head::Bare, word), |(head, head_word)| (head, head_word));
        let mut segment = Segment::new(head, head_word, word, number);
        segment.joint = Some(abbreviation("i.V.m.", first, last));
        self.joined.push(segment);
        self.evidence = Evidence::Confirmed;
    }

    /// Setzt ein Komma hinter das letzte Glied, falls dort keins steht
    /// („Absatz 1 Absatz 3“ → „I, III“).
    pub(super) fn separate(&mut self) {
        if let Some(word) = self
            .current_mut()
            .words
            .last_mut()
            .filter(|word| word.trail.is_empty())
        {
            word.trail.push(',');
        }
    }

    /// Das fertige Zitat; `None`, wenn eine bloße Zahl doch keine Norm war.
    /// Eine Norm ohne eigenes Gesetz zählt wie das nächste Gesetz danach.
    pub(super) fn finish(self) -> Option<Vec<Word>> {
        match self.first.head {
            Head::Bare => match self.evidence {
                Evidence::Pending => return None,
                Evidence::Confirmed => {}
            },
            Head::Paragraph | Head::Paragraphs | Head::Article => {}
        }
        let mut unit = None;
        let mut rendered: Vec<Vec<Word>> = Vec::with_capacity(self.joined.len() + 1);
        for segment in self.joined.into_iter().rev() {
            unit = segment.unit.or(unit);
            rendered.push(segment.render(Position::Joined, unit));
        }
        unit = self.first.unit.or(unit);
        rendered.push(self.first.render(Position::First, unit));
        Some(rendered.into_iter().rev().flatten().collect())
    }
}

/// „§“, „§§“ oder „Art.“.
fn sign(head: Head, count: Count, unit: Option<Unit>) -> Word {
    let article = || Word::new("Art", ".", " ");
    let paragraph = || match count {
        Count::One => Word::new("§", "", " "),
        Count::Several => Word::new("§§", "", " "),
    };
    match head {
        Head::Article => article(),
        Head::Paragraphs => Word::new("§§", "", " "),
        Head::Paragraph => paragraph(),
        Head::Bare => match unit {
            Some(Unit::Article) => article(),
            Some(Unit::Paragraph) | None => paragraph(),
        },
    }
}

/// Wie ein Normteil geschrieben wird: Wörter als (Text, Satzzeichen).
/// `previous` ist das Glied davor (nach einem Absatz ist ein Satz römisch
/// nur die Zahl: „II 1“).
fn part_text(part: Part, previous: Last, style: CitationStyle) -> Vec<(String, &'static str)> {
    let labelled = |label: &str, text: String| vec![(label.to_owned(), "."), (text, "")];
    match part {
        Part::Letter(letter) => labelled("lit", letter.to_string()),
        Part::Numbered { level, value } => {
            let text = value.to_string();
            match level {
                Subdivision::Subsection => match style {
                    CitationStyle::Roman => numbers::roman(value)
                        .map_or_else(|| labelled("Abs", text), |roman| vec![(roman, "")]),
                    CitationStyle::Abbreviated => labelled("Abs", text),
                },
                Subdivision::Sentence => match style {
                    CitationStyle::Roman if previous.after_subsection() => vec![(text, "")],
                    CitationStyle::Roman | CitationStyle::Abbreviated => labelled("S", text),
                },
                Subdivision::Subparagraph => labelled("UAbs", text),
                Subdivision::HalfSentence => labelled("Hs", text),
                Subdivision::Item => labelled("Nr", text),
                Subdivision::Alternative => labelled("Alt", text),
                Subdivision::Variant => labelled("Var", text),
                Subdivision::Case => vec![("Fall".to_owned(), ""), (text, "")],
            }
        }
    }
}

/// Eine Zahl nach „und“ auf der Stufe davor („Absatz 2 und 3“ → „II und III“).
fn same_level_text(level: Subdivision, value: u32, style: CitationStyle) -> String {
    match level {
        Subdivision::Subsection => match style {
            CitationStyle::Roman => numbers::roman(value).unwrap_or_else(|| value.to_string()),
            CitationStyle::Abbreviated => value.to_string(),
        },
        Subdivision::Subparagraph
        | Subdivision::Sentence
        | Subdivision::HalfSentence
        | Subdivision::Item
        | Subdivision::Alternative
        | Subdivision::Variant
        | Subdivision::Case => value.to_string(),
    }
}

/// Ein Wort mit neuem Text und den Satzzeichen und dem Leerraum von
/// `source` (ohne Satzzeichen davor).
pub(super) fn rendered(core: &str, source: &Word) -> Word {
    Word::new(core, &source.trail, &source.space)
}

/// Eine Abkürzung wie „ff.“ oder „i.V.m.“ anstelle der Wörter `first` bis
/// `last`; ein Punkt am Ende von `last` wird nicht verdoppelt.
pub(super) fn abbreviation(text: &str, first: &Word, last: &Word) -> Word {
    let (core, dot) = text
        .strip_suffix('.')
        .map_or((text, ""), |core| (core, "."));
    let rest = match dot {
        "." => last.trail.strip_prefix('.').unwrap_or(&last.trail),
        _ => &last.trail,
    };
    Word {
        lead: first.lead.clone(),
        ..Word::new(core, &format!("{dot}{rest}"), &last.space)
    }
}
