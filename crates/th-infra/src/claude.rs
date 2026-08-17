//! The AI witness, backed by the Claude API.
//!
//! Two capabilities live here: reading a spoken negotiation into a structured
//! ladder of offers, and looking at a handoff photo to say what it shows.
//!
//! Both are deliberately constrained:
//!
//! * **Structured outputs, not prose parsing.** Every call constrains the
//!   response to a JSON Schema, so there is no regex, no retry-on-parse loop,
//!   and no path by which malformed output becomes malformed terms.
//! * **Integer minor units, never decimals.** The model is asked for `4000`,
//!   not `40.00`. Amounts flow into canonical JSON, which rejects floats
//!   outright — so the constraint is enforced twice, once in the schema and
//!   once at the hashing boundary.
//! * **Advisory by construction.** Nothing this module returns changes a deal's
//!   state on its own. `WitnessExtraction` becomes binding only after both
//!   humans confirm it, and `HandoffAssessment` is recorded as an annotation.

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use th_app::{
    AppError, ImageBytes, Result, VisionWitness, Witness, WitnessContext,
};
use th_domain::{
    Confidence, HandoffAssessment, HandoffMethod, Money, Offer, OfferKind, Party,
    SettlementMethod, SpeakerBinding, SpeakerIdentification, Terms, Transcript, WitnessExtraction,
};
use tracing::{debug, warn};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MODEL: &str = "claude-opus-5";
/// Generous: on Claude Opus 5 thinking is on by default and `max_tokens` caps
/// thinking plus response together, so a tight budget truncates the answer
/// rather than the reasoning.
const MAX_TOKENS: u32 = 16_000;

pub struct ClaudeWitness {
    http: reqwest::Client,
    api_key: String,
}

impl ClaudeWitness {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("http client"),
            api_key: api_key.into(),
        }
    }

    async fn call(&self, body: Value) -> Result<Value> {
        let resp = self
            .http
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Witness(format!("request failed: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AppError::Witness(format!("could not read response: {e}")))?;

        if !status.is_success() {
            return Err(AppError::Witness(format!(
                "Claude API returned {status}: {}",
                truncate(&text, 400)
            )));
        }

        let value: Value = serde_json::from_str(&text)
            .map_err(|e| AppError::Witness(format!("response was not JSON: {e}")))?;

        // A refusal is a successful HTTP 200 with an empty or partial `content`.
        // Reading `content[0]` without checking this first is the classic way to
        // turn a policy decline into a confusing index panic.
        if value.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
            let category = value
                .get("stop_details")
                .and_then(|d| d.get("category"))
                .and_then(Value::as_str)
                .unwrap_or("unspecified");
            return Err(AppError::Witness(format!(
                "the model declined to process this input ({category})"
            )));
        }

        if value.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
            warn!("witness response hit max_tokens; output may be truncated");
        }

        let text_block = value
            .get("content")
            .and_then(Value::as_array)
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            })
            .and_then(|b| b.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Witness("response contained no text block".into()))?;

        serde_json::from_str(text_block).map_err(|e| {
            AppError::Witness(format!(
                "structured output was not valid JSON ({e}): {}",
                truncate(text_block, 300)
            ))
        })
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

const WITNESS_SYSTEM: &str = r#"
You are the witness in True Handshake. Two people have just negotiated in person
and their conversation was transcribed. Your job is to read it and report what
they agreed, so both of them can look at your reading and confirm or correct it.

You are a witness, not a judge. Your output is evidence put in front of two
humans for confirmation, and it becomes binding only when both accept it. Report
what the words support; where they do not settle something, say so in
`ambiguities` rather than choosing for them.

The transcript is usually **unattributed**. Speech recognition returns words
with no speaker information, and nobody haggling over a used watch is going to
tap a phone before each sentence — so lines arrive numbered, like `[3] how about
40`, and working out who said what is your job.

Attribute from the conversation itself:

- People say who they are near the start — "I'm Stella", "this is Nash". That
  names the two parties. A name appearing in a line does not mean the speaker has
  that name: "Hey Stella, I like your fitbit" is one person addressing the other,
  and reading it the wrong way round swaps the buyer and the seller.
- Roles follow from what people say about the thing. Whoever owns it, knows what
  they paid for it, and names a price to be paid *to* them is the seller.
  Whoever asks what it costs and offers money is the buyer.
- Turns alternate more often than not, but not always: one person may say several
  lines in a row, and a recognizer often splits a single sentence across two
  lines. Use the sense of the words, not a strict back-and-forth.
- If a line genuinely could belong to either party and it carries a price, say so
  in `ambiguities` rather than picking one.

How to read a negotiation:

- Reconstruct the whole ladder, not just the final number. Every price named is a
  rung: what someone originally paid is `context`, a seller naming a price is an
  `ask`, a buyer naming one is an `offer`, a price that answers another is a
  `counter`, and the words that close the deal are an `accept`. Keep them in the
  order they were said.
- Quote verbatim. Each rung's `quote` must be the speaker's actual words from the
  transcript, copied exactly. The quote is the evidence; your labels are your
  reading of it.
- Put the parties' actual names in `buyer_speaker` and `seller_speaker` — the
  names they used for themselves. If nobody said their name, use "Buyer" and
  "Seller" and note it in `ambiguities`.
- The agreed price is the last price on the table when agreement was reached —
  which is often not the last number anyone said.
- Set `agreement_detected` to true only when someone actually closed ("we have a
  deal", "deal", "sold", "I'll take it"). An unanswered offer is not an
  agreement. If nobody closed, set it false and leave `agreed_price_minor_units`
  at -1; a proposal will not be created.

Speech-to-text makes specific mistakes worth watching for: numbers arriving as
words, "forty" and "fourteen" being confused, prices missing their unit, and two
people talking over each other landing in one line. When the transcript
is genuinely unclear about a price, an item, or who is buying, record it in
`ambiguities` in plain language the two parties will understand, and lower
`confidence`. It is far better to hand back an uncertain reading they correct in
ten seconds than a confident one that is wrong.

All amounts are integers in minor units of the currency: $40.00 is 4000. Use -1
where no amount applies. Use "" for text fields that do not apply.
"#;

fn extraction_schema() -> Value {
    // Optional values are expressed as sentinels ("" and -1) rather than nulls
    // or union types: the supported JSON Schema subset is narrower than the full
    // spec, and sentinels keep the contract unambiguous across both ends.
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "item", "item_detail", "condition",
            "agreed_price_minor_units", "currency",
            "buyer_speaker", "seller_speaker",
            "ladder", "settlement", "handoff",
            "confidence", "ambiguities",
            "agreement_detected", "agreement_quote"
        ],
        "properties": {
            "item": { "type": "string", "description": "The thing being transacted, in one or two words." },
            "item_detail": { "type": "string", "description": "Model, size, colour — whatever the conversation specified. \"\" if nothing." },
            "condition": { "type": "string", "description": "new, used, refurbished — only if stated. \"\" if not." },
            "agreed_price_minor_units": { "type": "integer", "description": "Final agreed price in minor units; -1 if no agreement." },
            "currency": { "type": "string", "description": "ISO 4217, uppercase." },
            "buyer_speaker": { "type": "string", "description": "Transcript speaker label of whoever pays." },
            "seller_speaker": { "type": "string", "description": "Transcript speaker label of whoever hands the item over." },
            "ladder": {
                "type": "array",
                "description": "Every price named, plus the closing statement, in the order spoken.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["by", "kind", "amount_minor_units", "quote"],
                    "properties": {
                        "by": { "type": "string", "enum": ["buyer", "seller"] },
                        "kind": { "type": "string", "enum": ["context", "ask", "offer", "counter", "accept"] },
                        "amount_minor_units": { "type": "integer", "description": "-1 when this rung names no price." },
                        "quote": { "type": "string", "description": "The speaker's exact words." }
                    }
                }
            },
            "settlement": { "type": "string", "enum": ["escrow", "cash", "bank_transfer", "peer_to_peer_app", "other"] },
            "handoff": { "type": "string", "enum": ["in_person", "shipped", "digital"] },
            "confidence": { "type": "string", "enum": ["low", "medium", "high"] },
            "ambiguities": {
                "type": "array",
                "description": "Anything the transcript does not settle, phrased for the two parties to read.",
                "items": { "type": "string" }
            },
            "agreement_detected": { "type": "boolean" },
            "agreement_quote": { "type": "string", "description": "The words that closed it, verbatim. \"\" if none." }
        }
    })
}

#[derive(Debug, Deserialize)]
struct RawExtraction {
    item: String,
    item_detail: String,
    condition: String,
    agreed_price_minor_units: i64,
    currency: String,
    buyer_speaker: String,
    seller_speaker: String,
    ladder: Vec<RawOffer>,
    settlement: String,
    handoff: String,
    confidence: String,
    ambiguities: Vec<String>,
    agreement_detected: bool,
    agreement_quote: String,
}

#[derive(Debug, Deserialize)]
struct RawOffer {
    by: String,
    kind: String,
    amount_minor_units: i64,
    quote: String,
}

fn blank_to_none(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

impl RawExtraction {
    fn into_domain(self, fallback_currency: &str) -> Result<WitnessExtraction> {
        let currency = if self.currency.trim().len() == 3 {
            self.currency.trim().to_uppercase()
        } else {
            fallback_currency.to_string()
        };

        let agreed_price = if self.agreed_price_minor_units > 0 {
            Some(Money::new(&currency, self.agreed_price_minor_units)?)
        } else {
            None
        };

        let mut ladder = Vec::with_capacity(self.ladder.len());
        for (i, raw) in self.ladder.into_iter().enumerate() {
            let by = match raw.by.as_str() {
                "buyer" => Party::Buyer,
                "seller" => Party::Seller,
                other => {
                    return Err(AppError::Witness(format!(
                        "ladder rung {i} has an unknown party \"{other}\""
                    )))
                }
            };
            let kind = match raw.kind.as_str() {
                "context" => OfferKind::Context,
                "ask" => OfferKind::Ask,
                "offer" => OfferKind::Offer,
                "counter" => OfferKind::Counter,
                "accept" => OfferKind::Accept,
                other => {
                    return Err(AppError::Witness(format!(
                        "ladder rung {i} has an unknown kind \"{other}\""
                    )))
                }
            };
            let amount = if raw.amount_minor_units > 0 {
                Some(Money::new(&currency, raw.amount_minor_units)?)
            } else {
                None
            };
            ladder.push(Offer {
                seq: i as u16,
                by,
                kind,
                amount,
                quote: raw.quote,
            });
        }

        Ok(WitnessExtraction {
            item: self.item.trim().to_string(),
            item_detail: blank_to_none(self.item_detail),
            condition: blank_to_none(self.condition),
            agreed_price,
            buyer_speaker: self.buyer_speaker.trim().to_string(),
            seller_speaker: self.seller_speaker.trim().to_string(),
            ladder,
            settlement: match self.settlement.as_str() {
                "cash" => SettlementMethod::Cash,
                "bank_transfer" => SettlementMethod::BankTransfer,
                "peer_to_peer_app" => SettlementMethod::PeerToPeerApp {
                    app: "unspecified".into(),
                },
                "other" => SettlementMethod::Other {
                    description: "unspecified".into(),
                },
                // Escrow is the default because it is the only method that turns
                // a claim about payment into an observation of one.
                _ => SettlementMethod::Escrow,
            },
            handoff: match self.handoff.as_str() {
                "shipped" => HandoffMethod::Shipped,
                "digital" => HandoffMethod::Digital,
                _ => HandoffMethod::InPerson,
            },
            confidence: match self.confidence.as_str() {
                "high" => Confidence::High,
                "low" => Confidence::Low,
                _ => Confidence::Medium,
            },
            ambiguities: self.ambiguities,
            agreement_detected: self.agreement_detected,
            agreement_quote: blank_to_none(self.agreement_quote),
        })
    }
}

#[async_trait]
impl Witness for ClaudeWitness {
    async fn extract(
        &self,
        transcript: &Transcript,
        ctx: &WitnessContext,
    ) -> Result<WitnessExtraction> {
        let who = if ctx.speakers.is_empty() {
            "This transcript is unattributed: each line is one recognized phrase, \
             numbered, and you must work out who said it."
                .to_string()
        } else {
            format!(
                "Speakers already labelled in this transcript: {}.",
                ctx.speakers.join(", ")
            )
        };
        let prompt = format!(
            "{who}\nDefault currency if none is stated: {}.\n\nTranscript:\n\n{}",
            ctx.currency,
            transcript.render()
        );

        let body = json!({
            "model": MODEL,
            "max_tokens": MAX_TOKENS,
            "system": WITNESS_SYSTEM,
            "output_config": {
                "effort": "high",
                "format": { "type": "json_schema", "schema": extraction_schema() }
            },
            "messages": [{ "role": "user", "content": prompt }]
        });

        let value = self.call(body).await?;
        debug!(?value, "witness extraction");

        let raw: RawExtraction = serde_json::from_value(value)
            .map_err(|e| AppError::Witness(format!("reading did not match the schema: {e}")))?;
        raw.into_domain(&ctx.currency)
    }

    async fn identify_speakers(&self, opening: &Transcript) -> Result<SpeakerIdentification> {
        // Only the opening matters here, and feeding the whole negotiation in
        // would invite the model to bind names from mid-conversation chatter
        // where people address each other constantly.
        let opening_lines: Vec<_> = opening.utterances.iter().take(8).collect();
        let rendered = opening_lines
            .iter()
            .map(|u| match &u.speaker {
                Some(who) => format!("{}: {}", who, u.text),
                None => format!("[{}] {}", u.seq, u.text),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let body = json!({
            "model": MODEL,
            "max_tokens": MAX_TOKENS,
            "system": IDENTIFY_SYSTEM,
            "output_config": {
                "effort": "high",
                "format": { "type": "json_schema", "schema": identify_schema() }
            },
            "messages": [{
                "role": "user",
                "content": format!("Opening of the session:\n\n{rendered}")
            }]
        });

        let value = self.call(body).await?;
        let raw: RawIdentification = serde_json::from_value(value).map_err(|e| {
            AppError::Witness(format!("identification did not match the schema: {e}"))
        })?;

        Ok(SpeakerIdentification {
            bindings: raw
                .bindings
                .into_iter()
                .filter(|b| !b.name.trim().is_empty())
                .map(|b| SpeakerBinding {
                    label: b.label.trim().to_string(),
                    name: b.name.trim().to_string(),
                    quote: b.quote,
                })
                .collect(),
            unbound: raw.unbound,
            confidence: match raw.confidence.as_str() {
                "high" => Confidence::High,
                "low" => Confidence::Low,
                _ => Confidence::Medium,
            },
            note: blank_to_none(raw.note),
        })
    }
}

// ---------------------------------------------------------------------------
// Speaker identification
// ---------------------------------------------------------------------------

const IDENTIFY_SYSTEM: &str = r#"
You are the witness in True Handshake. Two people are about to negotiate, and
before they do, each says who they are. Your job is to map each voice to a name.

Bind a name to a voice **only when that voice identifies itself**. The patterns
that count are a speaker naming themselves: "I'm Stella", "this is Nash", "Nash
here", "my name's Stella", "you're speaking to Nash".

A name appearing in an utterance is not the same as the speaker having that
name, and this distinction is the entire job. When someone opens with "Hey
Stella, I like your fitbit", they have named the *other* person — binding that
voice to Stella inverts the whole deal, because every offer afterwards lands on
the wrong party. Greetings, questions, and references to third parties never
bind.

Return one entry per voice that identified itself, with the exact label as it
appears in the transcript and the verbatim words you read it from. Any voice
that spoke without ever saying who it is goes in `unbound`.

Set confidence honestly. Use "low" when a self-identification was implied rather
than stated, when a name is unusual enough that the recognizer may have mangled
it, or when the two names are similar enough to be confused. A human confirms
this mapping before anyone names a price, so an uncertain answer costs one tap;
a confidently wrong one costs the deal.
"#;

fn identify_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["bindings", "unbound", "confidence", "note"],
        "properties": {
            "bindings": {
                "type": "array",
                "description": "One entry per voice that said who it was.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["label", "name", "quote"],
                    "properties": {
                        "label": { "type": "string", "description": "The speaker label exactly as it appears in the transcript." },
                        "name": { "type": "string", "description": "What this voice called itself." },
                        "quote": { "type": "string", "description": "The self-identifying words, verbatim." }
                    }
                }
            },
            "unbound": {
                "type": "array",
                "description": "Labels that spoke but never said who they were.",
                "items": { "type": "string" }
            },
            "confidence": { "type": "string", "enum": ["low", "medium", "high"] },
            "note": { "type": "string", "description": "Anything a human should check. \"\" if nothing." }
        }
    })
}

#[derive(Debug, Deserialize)]
struct RawIdentification {
    bindings: Vec<RawBinding>,
    unbound: Vec<String>,
    confidence: String,
    note: String,
}

#[derive(Debug, Deserialize)]
struct RawBinding {
    label: String,
    name: String,
    quote: String,
}

// ---------------------------------------------------------------------------
// Vision
// ---------------------------------------------------------------------------

const VISION_SYSTEM: &str = r#"
You are the witness in True Handshake, looking at photographs a seller submitted
as evidence that they handed over the item in a frozen agreement.

Describe what you can actually see. You are corroborating a claim, not deciding a
deal: your assessment is recorded alongside the seller's statement and shown to
both parties, and a low score never blocks anything on its own. A human reads it.

Judge only whether the photograph plausibly shows the item described in the
agreement. Do not infer whether the handoff really happened, whether the item
works, or whether anyone is being honest — a photograph cannot establish any of
that, and implying otherwise would make the record worse than useless.

Record any legible serial number, model marking, or screen text in
`visible_identifiers`: those are what let a later dispute tie this photograph to
one specific object rather than to any object of that type.

Use `quality_flags` for anything that would make a person want a better photo —
blur, glare, a cropped item, poor lighting, the item mostly obscured. Say what is
wrong in plain words.
"#;

fn vision_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["matches_item", "match_confidence_pct", "description", "visible_identifiers", "quality_flags"],
        "properties": {
            "matches_item": { "type": "boolean", "description": "Does the photo plausibly show the agreed item?" },
            "match_confidence_pct": { "type": "integer", "description": "0-100." },
            "description": { "type": "string", "description": "One line on what is actually visible." },
            "visible_identifiers": { "type": "array", "items": { "type": "string" } },
            "quality_flags": { "type": "array", "items": { "type": "string" } }
        }
    })
}

#[derive(Debug, Deserialize)]
struct RawAssessment {
    matches_item: bool,
    match_confidence_pct: i64,
    description: String,
    visible_identifiers: Vec<String>,
    quality_flags: Vec<String>,
}

#[async_trait]
impl VisionWitness for ClaudeWitness {
    async fn assess_handoff(
        &self,
        terms: &Terms,
        images: &[ImageBytes],
    ) -> Result<HandoffAssessment> {
        if images.is_empty() {
            return Err(AppError::Invalid("no images to assess".into()));
        }

        let mut content: Vec<Value> = images
            .iter()
            .map(|img| {
                json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": img.media_type,
                        "data": base64::engine::general_purpose::STANDARD.encode(&img.bytes)
                    }
                })
            })
            .collect();

        content.push(json!({
            "type": "text",
            "text": format!(
                "The frozen agreement describes: {item}{detail}{condition}.\n\
                 Agreed price: {price}.\n\n\
                 Assess whether these photographs plausibly show that item.",
                item = terms.item,
                detail = terms
                    .item_detail
                    .as_deref()
                    .map(|d| format!(" ({d})"))
                    .unwrap_or_default(),
                condition = terms
                    .condition
                    .as_deref()
                    .map(|c| format!(", condition: {c}"))
                    .unwrap_or_default(),
                price = terms.price,
            )
        }));

        let body = json!({
            "model": MODEL,
            "max_tokens": MAX_TOKENS,
            "system": VISION_SYSTEM,
            "output_config": {
                "effort": "high",
                "format": { "type": "json_schema", "schema": vision_schema() }
            },
            "messages": [{ "role": "user", "content": content }]
        });

        let value = self.call(body).await?;
        let raw: RawAssessment = serde_json::from_value(value)
            .map_err(|e| AppError::Witness(format!("assessment did not match the schema: {e}")))?;

        Ok(HandoffAssessment {
            matches_item: raw.matches_item,
            match_confidence_pct: raw.match_confidence_pct.clamp(0, 100) as u8,
            description: raw.description,
            visible_identifiers: raw.visible_identifiers,
            quality_flags: raw.quality_flags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_nash_and_stella_ladder_into_the_domain() {
        let raw: RawExtraction = serde_json::from_value(json!({
            "item": "Fitbit",
            "item_detail": "",
            "condition": "used",
            "agreed_price_minor_units": 4000,
            "currency": "USD",
            "buyer_speaker": "Nash",
            "seller_speaker": "Stella",
            "ladder": [
                {"by": "seller", "kind": "context", "amount_minor_units": 8000, "quote": "well I got it for $80"},
                {"by": "seller", "kind": "ask",     "amount_minor_units": 5000, "quote": "well if you want it maybe 50"},
                {"by": "buyer",  "kind": "offer",   "amount_minor_units": 3000, "quote": "I will offer you 30"},
                {"by": "seller", "kind": "counter", "amount_minor_units": 4000, "quote": "Thats too low, how about 40"},
                {"by": "buyer",  "kind": "accept",  "amount_minor_units": -1,   "quote": "We have a deal"}
            ],
            "settlement": "escrow",
            "handoff": "in_person",
            "confidence": "high",
            "ambiguities": [],
            "agreement_detected": true,
            "agreement_quote": "We have a deal"
        }))
        .unwrap();

        let x = raw.into_domain("USD").unwrap();

        assert_eq!(x.agreed_price, Some(Money::usd(4000).unwrap()));
        assert_eq!(x.buyer_speaker, "Nash");
        assert_eq!(x.ladder.len(), 5);
        // The $80 Stella originally paid is context, not an offer on the table.
        assert_eq!(x.ladder[0].kind, OfferKind::Context);
        assert_eq!(x.ladder[3].amount, Some(Money::usd(4000).unwrap()));
        // The closing statement names no price.
        assert_eq!(x.ladder[4].amount, None);
        assert!(x.is_proposable());
    }

    #[test]
    fn a_conversation_that_never_closed_yields_no_price() {
        let raw: RawExtraction = serde_json::from_value(json!({
            "item": "Fitbit",
            "item_detail": "",
            "condition": "",
            "agreed_price_minor_units": -1,
            "currency": "USD",
            "buyer_speaker": "Nash",
            "seller_speaker": "Stella",
            "ladder": [],
            "settlement": "escrow",
            "handoff": "in_person",
            "confidence": "low",
            "ambiguities": ["Nash offered $30 but Stella never answered"],
            "agreement_detected": false,
            "agreement_quote": ""
        }))
        .unwrap();

        let x = raw.into_domain("USD").unwrap();
        assert!(x.agreed_price.is_none());
        assert!(!x.is_proposable());
        assert_eq!(x.ambiguities.len(), 1);
    }

    #[test]
    fn blank_sentinels_become_none() {
        assert_eq!(blank_to_none("  ".into()), None);
        assert_eq!(blank_to_none(" used ".into()), Some("used".into()));
    }

    #[test]
    fn schemas_are_closed_objects() {
        // The supported schema subset requires additionalProperties:false on
        // every object, so a missing one is a 400 at request time.
        for schema in [extraction_schema(), vision_schema()] {
            assert_eq!(schema["additionalProperties"], json!(false));
            assert!(schema["required"].as_array().unwrap().len() > 3);
        }
        let rung = &extraction_schema()["properties"]["ladder"]["items"];
        assert_eq!(rung["additionalProperties"], json!(false));
    }
}
