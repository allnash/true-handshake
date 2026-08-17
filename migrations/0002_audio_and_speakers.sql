-- Audio evidence and speaker binding.
--
-- Until now the receipt could prove the *transcript* was not altered. It could
-- not prove the transcript matched what was actually said, because the audio
-- was discarded the moment the recognizer returned text. Storing the recording
-- and hashing it into the attestation closes that gap: the chain now commits to
-- the sound, not just to our reading of it.
--
-- The bytes live outside the chain, referenced by handle, so a recording can be
-- destroyed on request without breaking a single receipt — the hash stays, and a
-- destroyed recording simply stops being checkable.

alter table witness_sessions
    add column audio_ref        text,
    add column audio_sha256     text,
    -- Which voice belongs to whom, bound from what people said about
    -- themselves, then confirmed by a human before the negotiation starts.
    add column speaker_bindings jsonb;

create table audio_objects (
    reference    text primary key,
    deal_id      uuid        not null references deals (id) on delete cascade,
    media_type   text        not null,
    -- Recomputed server-side. A client-supplied hash would let anyone claim a
    -- recording says whatever they like.
    sha256       text        not null,
    bytes        bytea       not null,
    duration_ms  integer,
    created_at   timestamptz not null
);

create index audio_objects_deal_idx on audio_objects (deal_id);
