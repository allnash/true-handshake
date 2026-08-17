//! A witness that runs without an API key.
//!
//! This exists so `cargo run` works on a fresh clone and so the lifecycle tests
//! do not depend on a network call. It reads prices out of a transcript with
//! plain string scanning: good enough to walk the demo, and deliberately dumb
//! enough that nobody mistakes it for the real thing. It reports
//! `Confidence::Low` and says so in `ambiguities` on every reading.

use async_trait::async_trait;
use th_app::{ImageBytes, Result, VisionWitness, Witness, WitnessContext};
use th_domain::{
    Confidence, HandoffAssessment, HandoffMethod, Money, Offer, OfferKind, Party, SettlementMethod,
    SpeakerBinding, SpeakerIdentification, Terms, Transcript, WitnessExtraction,
};

pub struct OfflineWitness;

/// Pull every price-looking number out of a line, in order.
///
/// Handles `$80`, `80`, `$1,250` and `40.00`. Returns minor units.
fn scan_amounts(text: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut whole = String::new();
            let mut cents: Option<String> = None;

            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == ',') {
                if bytes[i].is_ascii_digit() {
                    whole.push(bytes[i]);
                }
                i += 1;
            }
            // A decimal point followed by exactly two digits is cents.
            if i + 2 < bytes.len()
                && bytes[i] == '.'
                && bytes[i + 1].is_ascii_digit()
                && bytes[i + 2].is_ascii_digit()
            {
                cents = Some(format!("{}{}", bytes[i + 1], bytes[i + 2]));
                i += 3;
            }

            // Skip things that are clearly not prices: years, ordinals.
            let preceded_by_dollar = start > 0 && bytes[start - 1] == '$';
            let plausible = preceded_by_dollar || whole.len() <= 4;
            if plausible {
                if let Ok(major) = whole.parse::<i64>() {
                    let minor = major * 100 + cents.and_then(|c| c.parse::<i64>().ok()).unwrap_or(0);
                    if minor > 0 {
                        out.push(minor);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

fn mentions_agreement(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "we have a deal",
        "it's a deal",
        "its a deal",
        "deal",
        "sold",
        "i'll take it",
        "ill take it",
        "agreed",
    ]
    .iter()
    .any(|needle| t.contains(needle))
}

#[async_trait]
impl Witness for OfflineWitness {
    async fn extract(
        &self,
        transcript: &Transcript,
        ctx: &WitnessContext,
    ) -> Result<WitnessExtraction> {
        let speakers = transcript.speakers();
        // Whoever speaks first is guessed to be the buyer, and unattributed lines
        // are assumed to alternate. Both are coin flips dressed up as rules,
        // which is exactly why this witness reports low confidence and why both
        // parties confirm before anything binds.
        let buyer = speakers.first().cloned().unwrap_or_else(|| "Buyer".into());
        let seller = speakers.get(1).cloned().unwrap_or_else(|| "Seller".into());

        let mut ladder: Vec<Offer> = Vec::new();
        let mut agreed: Option<i64> = None;
        let mut agreement_quote = None;

        for (turn, u) in transcript.utterances.iter().enumerate() {
            let by = match &u.speaker {
                Some(label) if *label == buyer => Party::Buyer,
                Some(_) => Party::Seller,
                // Nothing to go on: alternate, and say so in `ambiguities`.
                None if turn % 2 == 0 => Party::Buyer,
                None => Party::Seller,
            };

            for amount in scan_amounts(&u.text) {
                let kind = match (by, ladder.is_empty()) {
                    (Party::Seller, true) => OfferKind::Context,
                    (Party::Seller, false) => {
                        if ladder.iter().any(|o| o.by == Party::Buyer) {
                            OfferKind::Counter
                        } else {
                            OfferKind::Ask
                        }
                    }
                    (Party::Buyer, _) => OfferKind::Offer,
                };
                ladder.push(Offer {
                    seq: ladder.len() as u16,
                    by,
                    kind,
                    amount: Some(Money::new(&ctx.currency, amount)?),
                    quote: u.text.trim().to_string(),
                });
            }

            if mentions_agreement(&u.text) {
                agreement_quote = Some(u.text.trim().to_string());
                // The agreed price is the last one on the table, which is not
                // necessarily the last number anyone said.
                agreed = ladder.last().and_then(|o| o.amount.as_ref()).map(|m| m.minor_units);
                ladder.push(Offer {
                    seq: ladder.len() as u16,
                    by,
                    kind: OfferKind::Accept,
                    amount: None,
                    quote: u.text.trim().to_string(),
                });
            }
        }

        Ok(WitnessExtraction {
            item: "item".into(),
            item_detail: None,
            condition: None,
            agreed_price: agreed
                .map(|a| Money::new(&ctx.currency, a))
                .transpose()?,
            buyer_speaker: buyer,
            seller_speaker: seller,
            ladder,
            settlement: SettlementMethod::Escrow,
            handoff: HandoffMethod::InPerson,
            confidence: Confidence::Low,
            ambiguities: vec![
                "This reading came from the offline witness, which scans for numbers and \
                 cannot understand what was said. Check the item, the price, and who is \
                 buying before you confirm."
                    .into(),
            ],
            agreement_detected: agreed.is_some(),
            agreement_quote,
        })
    }

    async fn identify_speakers(&self, opening: &Transcript) -> Result<SpeakerIdentification> {
        // Deliberately literal: match only self-identifying phrasings, never a
        // bare name. "Hey Stella, I like your fitbit" names the *other* party,
        // and binding on it would swap buyer and seller.
        const SELF: [&str; 6] = ["i am ", "i'm ", "im ", "this is ", "my name is ", "name's "];

        let mut bindings: Vec<SpeakerBinding> = Vec::new();
        let mut unbound: Vec<String> = Vec::new();

        for label in opening.speakers() {
            let mut found = None;
            for u in opening
                .utterances
                .iter()
                .filter(|u| u.speaker.as_deref() == Some(label.as_str()))
            {
                let lower = u.text.to_lowercase();
                for needle in SELF {
                    if let Some(at) = lower.find(needle) {
                        let rest = &u.text[at + needle.len()..];
                        let name: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '\'' || *c == '-')
                            .collect();
                        if name.len() >= 2 {
                            found = Some((name, u.text.trim().to_string()));
                            break;
                        }
                    }
                }
                if found.is_some() {
                    break;
                }
            }

            match found {
                Some((name, quote)) => bindings.push(SpeakerBinding {
                    label,
                    name: name.trim().to_string(),
                    quote,
                }),
                None => unbound.push(label),
            }
        }

        Ok(SpeakerIdentification {
            bindings,
            unbound,
            confidence: Confidence::Low,
            note: Some(
                "Read by the offline witness, which only matches phrases like \"I'm …\". \
                 Check the names before continuing."
                    .into(),
            ),
        })
    }
}

#[async_trait]
impl VisionWitness for OfflineWitness {
    async fn assess_handoff(
        &self,
        _terms: &Terms,
        images: &[ImageBytes],
    ) -> Result<HandoffAssessment> {
        Ok(HandoffAssessment {
            matches_item: false,
            match_confidence_pct: 0,
            description: format!(
                "{} photo(s) stored. The offline witness cannot see images, so nothing here \
                 has been checked against the agreement.",
                images.len()
            ),
            visible_identifiers: vec![],
            quality_flags: vec!["not assessed: offline witness".into()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use th_app::WitnessContext;
    use time::macros::datetime;

    fn transcript(lines: &[(&str, &str)]) -> Transcript {
        Transcript {
            utterances: lines
                .iter()
                .enumerate()
                .map(|(i, (speaker, text))| th_domain::Utterance {
                    seq: i as u32,
                    speaker: Some((*speaker).to_string()),
                    text: (*text).into(),
                    at: datetime!(2026-08-16 12:00:00 UTC),
                    confidence_pct: None,
                    corrected: false,
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn binds_voices_from_self_identification_only() {
        let t = transcript(&[
            // Nash names Stella here. Binding on a bare name would swap them.
            ("A", "Hey Stella - I like your fitbit"),
            ("B", "Hey, I am Stella"),
            ("A", "this is Nash by the way"),
        ]);

        let id = OfflineWitness.identify_speakers(&t).await.unwrap();
        assert_eq!(id.name_for("A"), Some("Nash"));
        assert_eq!(id.name_for("B"), Some("Stella"));
        assert!(id.is_complete());
    }

    #[tokio::test]
    async fn a_voice_that_never_says_who_it_is_stays_unbound() {
        let t = transcript(&[("A", "I'm Nash"), ("B", "how much for the fitbit")]);
        let id = OfflineWitness.identify_speakers(&t).await.unwrap();
        assert_eq!(id.unbound, vec!["B".to_string()]);
        assert!(!id.is_complete());
    }

    #[test]
    fn scans_prices_in_several_shapes() {
        assert_eq!(scan_amounts("well I got it for $80"), vec![8000]);
        assert_eq!(scan_amounts("maybe 50"), vec![5000]);
        assert_eq!(scan_amounts("$1,250 firm"), vec![125000]);
        assert_eq!(scan_amounts("that's 40.00 then"), vec![4000]);
        assert_eq!(scan_amounts("no numbers here"), Vec::<i64>::new());
    }

    #[tokio::test]
    async fn walks_the_nash_and_stella_negotiation() {
        let t = transcript(&[
            ("Nash", "Hey Stella - I like your fitbit, how much is it? if I want it"),
            ("Stella", "Hey Nash well I got it for $80"),
            ("Nash", "Well how much is it today?"),
            ("Stella", "Nash, well if you want it maybe 50"),
            ("Nash", "I will offer you 30"),
            ("Stella", "Thats too low, how about 40"),
            ("Nash", "We have a deal"),
        ]);

        let x = OfflineWitness
            .extract(
                &t,
                &WitnessContext {
                    speakers: t.speakers(),
                    currency: "USD".into(),
                },
            )
            .await
            .unwrap();

        assert_eq!(x.buyer_speaker, "Nash");
        assert_eq!(x.seller_speaker, "Stella");
        assert!(x.agreement_detected);
        // The last price on the table was Stella's $40, not the $80 she paid.
        assert_eq!(x.agreed_price, Some(Money::usd(4000).unwrap()));
        assert!(x.is_proposable());
        // And it is honest about being a dumb reading.
        assert_eq!(x.confidence, Confidence::Low);
        assert!(!x.ambiguities.is_empty());
    }

    #[tokio::test]
    async fn a_negotiation_with_no_close_is_not_proposable() {
        let t = transcript(&[
            ("Nash", "how much for the fitbit"),
            ("Stella", "maybe 50"),
            ("Nash", "I will offer you 30"),
        ]);

        let x = OfflineWitness
            .extract(
                &t,
                &WitnessContext {
                    speakers: t.speakers(),
                    currency: "USD".into(),
                },
            )
            .await
            .unwrap();

        assert!(!x.agreement_detected);
        assert!(!x.is_proposable());
    }
}
