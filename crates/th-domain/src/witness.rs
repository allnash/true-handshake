//! What the AI witness hears, and what it proposes.
//!
//! The load-bearing rule of this module: **the witness proposes, the humans
//! attest.** Nothing here is binding. A `WitnessExtraction` is a reading of a
//! conversation, and it becomes `Terms` only when both parties have looked at it
//! and confirmed it. That boundary is what keeps a mis-transcribed "$40" from
//! ever becoming a $40 obligation on its own.
//!
//! Note the absence of floating point. Speech-to-text and model confidences are
//! integer percentages, because the transcript is hashed into the receipt and
//! canonical JSON must not carry floats.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::money::Money;
use crate::terms::{HandoffMethod, Offer, Party, SettlementMethod, Terms};

/// One thing one person said, as captured by browser speech-to-text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utterance {
    pub seq: u32,
    /// Who said it, when that is known.
    ///
    /// Usually it is not. Browser speech recognition returns text with no
    /// speaker information, and asking two people mid-negotiation to tap a phone
    /// before each sentence is not something anyone will actually do. So the
    /// capture layer sends unattributed lines and the witness works out who is
    /// who from what was said — which the parties then confirm, exactly as they
    /// confirm the price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub text: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// Recognizer confidence, 0–100. Absent when the recognizer gave none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_pct: Option<u8>,
    /// True when the party edited the recognized text by hand.
    #[serde(default)]
    pub corrected: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub utterances: Vec<Utterance>,
}

impl Transcript {
    pub fn is_empty(&self) -> bool {
        self.utterances.is_empty()
    }

    /// Distinct speaker labels present, in first-appearance order. Empty when
    /// the transcript is unattributed, which is the normal case.
    pub fn speakers(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for u in &self.utterances {
            if let Some(s) = &u.speaker {
                if !names.iter().any(|n| n == s) {
                    names.push(s.clone());
                }
            }
        }
        names
    }

    pub fn is_attributed(&self) -> bool {
        !self.utterances.is_empty() && self.utterances.iter().all(|u| u.speaker.is_some())
    }

    /// Plain-text rendering handed to the model. Unattributed lines are numbered
    /// rather than labelled, so the model is never handed a speaker guess of
    /// ours and asked to agree with it.
    pub fn render(&self) -> String {
        self.utterances
            .iter()
            .map(|u| match &u.speaker {
                Some(who) => format!("{}: {}", who, u.text),
                None => format!("[{}] {}", u.seq, u.text),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// The witness's reading of a conversation. Advisory until both parties confirm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WitnessExtraction {
    pub item: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Absent when the conversation never reached a price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agreed_price: Option<Money>,
    /// Speaker labels from the transcript, mapped to roles.
    pub buyer_speaker: String,
    pub seller_speaker: String,
    pub ladder: Vec<Offer>,
    pub settlement: SettlementMethod,
    pub handoff: HandoffMethod,
    pub confidence: Confidence,
    /// Anything the witness could not resolve. Surfaced to both parties verbatim
    /// rather than guessed at.
    #[serde(default)]
    pub ambiguities: Vec<String>,
    /// Whether the parties actually closed, and the words that closed it.
    pub agreement_detected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agreement_quote: Option<String>,
}

impl WitnessExtraction {
    /// Whether this reading is complete enough to put in front of both parties.
    pub fn is_proposable(&self) -> bool {
        self.agreement_detected
            && self.agreed_price.is_some()
            && !self.item.trim().is_empty()
            && !self.buyer_speaker.trim().eq_ignore_ascii_case(self.seller_speaker.trim())
    }

    /// Turn a reading into candidate terms. Still not binding — this is what gets
    /// shown for confirmation.
    pub fn to_candidate_terms(&self) -> Option<Terms> {
        let price = self.agreed_price.clone()?;
        Some(Terms {
            item: self.item.clone(),
            item_detail: self.item_detail.clone(),
            condition: self.condition.clone(),
            price,
            buyer_name: self.buyer_speaker.clone(),
            seller_name: self.seller_speaker.clone(),
            settlement: self.settlement.clone(),
            handoff: self.handoff,
            ladder: self.ladder.clone(),
            notes: None,
        })
    }

    pub fn speaker_for(&self, party: Party) -> &str {
        match party {
            Party::Buyer => &self.buyer_speaker,
            Party::Seller => &self.seller_speaker,
        }
    }
}

// ---------------------------------------------------------------------------
// Speaker binding
// ---------------------------------------------------------------------------

/// One voice, tied to a name by what that voice said about itself.
///
/// The `label` is whatever the capture layer used to keep voices apart — a
/// diarizer's anonymous cluster id (`speaker_0`), or the name a party tapped on
/// screen. Binding is deliberately separate from separating: telling two voices
/// apart is a signal-processing job, and deciding which one is Stella is a
/// reading of what was said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerBinding {
    pub label: String,
    pub name: String,
    /// The self-identification this was read from, verbatim.
    pub quote: String,
}

/// The witness's reading of "who is who", before anyone has negotiated anything.
///
/// This runs on the opening seconds of a session — the moment where each party
/// says something like *"Hey, I'm Stella"* — and its whole job is to map voices
/// to names so that every later offer lands on the right person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerIdentification {
    pub bindings: Vec<SpeakerBinding>,
    /// Labels that spoke but never said who they were.
    #[serde(default)]
    pub unbound: Vec<String>,
    pub confidence: Confidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl SpeakerIdentification {
    /// Two voices, two distinct names, nobody left unbound.
    pub fn is_complete(&self) -> bool {
        self.bindings.len() == 2
            && self.unbound.is_empty()
            && !self.bindings[0]
                .name
                .trim()
                .eq_ignore_ascii_case(self.bindings[1].name.trim())
            && self.bindings.iter().all(|b| !b.name.trim().is_empty())
    }

    pub fn name_for(&self, label: &str) -> Option<&str> {
        self.bindings
            .iter()
            .find(|b| b.label == label)
            .map(|b| b.name.as_str())
    }

    /// Swap the two names. The confirmation screen offers exactly this, because
    /// the one failure mode worth designing for is the mapping being inverted.
    pub fn swapped(&self) -> Self {
        let mut next = self.clone();
        if next.bindings.len() == 2 {
            let a = next.bindings[0].name.clone();
            next.bindings[0].name = next.bindings[1].name.clone();
            next.bindings[1].name = a;
        }
        next
    }
}

/// A recording, described by its digest rather than its contents.
///
/// Only this struct enters the attestation payload. The bytes live behind a
/// reference and can be destroyed without breaking the chain — at which point
/// the receipt still states that a recording with this digest existed, and
/// simply stops being checkable against one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioEvidence {
    pub sha256: String,
    pub media_type: String,
    pub size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
}

/// The witness's assessment of a handoff photo. Also advisory — a low score
/// never blocks a deal on its own, it just gets recorded and shown to both sides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffAssessment {
    /// Does the image plausibly show the item named in the frozen terms?
    pub matches_item: bool,
    /// 0–100.
    pub match_confidence_pct: u8,
    /// What the witness actually sees, in one line.
    pub description: String,
    /// Serial numbers, model markings, screen text — anything legible that ties
    /// this photo to this specific object.
    #[serde(default)]
    pub visible_identifiers: Vec<String>,
    /// Blur, framing, lighting, obstruction — reasons a human should look closer.
    #[serde(default)]
    pub quality_flags: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn utterance(seq: u32, speaker: &str, text: &str) -> Utterance {
        Utterance {
            seq,
            speaker: Some(speaker.into()),
            text: text.into(),
            at: datetime!(2026-08-16 12:00:00 UTC),
            confidence_pct: Some(92),
            corrected: false,
        }
    }

    fn ident(a: (&str, &str), b: (&str, &str)) -> SpeakerIdentification {
        SpeakerIdentification {
            bindings: vec![
                SpeakerBinding { label: a.0.into(), name: a.1.into(), quote: format!("I'm {}", a.1) },
                SpeakerBinding { label: b.0.into(), name: b.1.into(), quote: format!("this is {}", b.1) },
            ],
            unbound: vec![],
            confidence: Confidence::High,
            note: None,
        }
    }

    #[test]
    fn a_binding_needs_two_distinct_named_voices() {
        assert!(ident(("speaker_0", "Stella"), ("speaker_1", "Nash")).is_complete());

        // Same person twice is not two parties.
        assert!(!ident(("speaker_0", "Nash"), ("speaker_1", "nash")).is_complete());

        let mut one_missing = ident(("speaker_0", "Stella"), ("speaker_1", ""));
        assert!(!one_missing.is_complete());
        one_missing.bindings.pop();
        assert!(!one_missing.is_complete());

        let mut stray = ident(("speaker_0", "Stella"), ("speaker_1", "Nash"));
        stray.unbound.push("speaker_2".into());
        assert!(!stray.is_complete(), "a third voice nobody named blocks the binding");
    }

    #[test]
    fn swapping_inverts_the_mapping_and_nothing_else() {
        let i = ident(("speaker_0", "Stella"), ("speaker_1", "Nash"));
        let s = i.swapped();
        assert_eq!(s.name_for("speaker_0"), Some("Nash"));
        assert_eq!(s.name_for("speaker_1"), Some("Stella"));
        // Quotes stay attached to the voice that actually said them.
        assert_eq!(s.bindings[0].quote, i.bindings[0].quote);
        assert_eq!(s.swapped(), i);
    }

    #[test]
    fn audio_evidence_carries_a_digest_not_bytes() {
        let a = AudioEvidence {
            sha256: "ab".repeat(32),
            media_type: "audio/webm".into(),
            size_bytes: 91_233,
            duration_ms: Some(41_500),
        };
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("sha256"));
        assert!(!json.contains("bytes\":["), "raw audio must never enter the payload");
    }

    #[test]
    fn an_unattributed_transcript_renders_without_inventing_speakers() {
        let line = |seq: u32, text: &str| Utterance {
            seq,
            speaker: None,
            text: text.into(),
            at: datetime!(2026-08-16 12:00:00 UTC),
            confidence_pct: None,
            corrected: false,
        };
        let t = Transcript {
            utterances: vec![line(0, "how much for the fitbit"), line(1, "I got it for $80")],
        };
        assert!(!t.is_attributed());
        assert_eq!(t.speakers(), Vec::<String>::new());
        assert_eq!(t.render(), "[0] how much for the fitbit\n[1] I got it for $80");
    }

    #[test]
    fn renders_and_lists_speakers_in_first_appearance_order() {
        let t = Transcript {
            utterances: vec![
                utterance(0, "Nash", "I like your fitbit, how much?"),
                utterance(1, "Stella", "I got it for $80"),
                utterance(2, "Nash", "how much today?"),
            ],
        };
        assert_eq!(t.speakers(), vec!["Nash", "Stella"]);
        assert_eq!(
            t.render(),
            "Nash: I like your fitbit, how much?\nStella: I got it for $80\nNash: how much today?"
        );
    }

    #[test]
    fn extraction_without_agreement_is_not_proposable() {
        let mut x = extraction();
        x.agreement_detected = false;
        assert!(!x.is_proposable());

        let mut x = extraction();
        x.agreed_price = None;
        assert!(!x.is_proposable());
    }

    #[test]
    fn candidate_terms_carry_the_ladder_through() {
        let x = extraction();
        let t = x.to_candidate_terms().unwrap();
        assert_eq!(t.price, Money::usd(4000).unwrap());
        assert_eq!(t.buyer_name, "Nash");
        assert_eq!(t.seller_name, "Stella");
    }

    fn extraction() -> WitnessExtraction {
        WitnessExtraction {
            item: "Fitbit".into(),
            item_detail: None,
            condition: Some("used".into()),
            agreed_price: Some(Money::usd(4000).unwrap()),
            buyer_speaker: "Nash".into(),
            seller_speaker: "Stella".into(),
            ladder: vec![],
            settlement: SettlementMethod::Escrow,
            handoff: HandoffMethod::InPerson,
            confidence: Confidence::High,
            ambiguities: vec![],
            agreement_detected: true,
            agreement_quote: Some("We have a deal".into()),
        }
    }
}
