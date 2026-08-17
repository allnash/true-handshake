-- True Handshake, initial schema.
--
-- Two things to notice:
--
--   * `attestations (deal_id, seq)` is unique. That constraint is the
--     concurrency backstop for the whole trust ledger: two racing writers
--     cannot both append seq N, so a chain can never fork.
--
--   * Handoff photos live in `proof_objects`, referenced by an opaque handle.
--     No image bytes and no personal data ever enter `attestations.payload`,
--     because that payload is hashed into the public receipt. Destroying a
--     proof object leaves the chain intact and verifiable.

create table deals (
    id                      uuid primary key,
    state                   text        not null,
    -- Only set when `state = 'resolved'`; carries which way the mediation went.
    dispute_outcome         text,
    version                 integer     not null,
    terms_revision          integer     not null,
    buyer_confirmed         integer,
    seller_confirmed        integer,
    terms                   jsonb,
    terms_hash              text,
    evidence_tier           text        not null,
    receipt_auto_confirmed  boolean     not null default false,
    created_at              timestamptz not null,
    frozen_at               timestamptz,
    release_due_at          timestamptz,
    terminal_at             timestamptz,

    -- v1 identity: a per-deal bearer token per side. Not authentication; see
    -- PartyBinding in th-app for the honest description of what this is.
    buyer_name              text        not null,
    seller_name             text        not null,
    buyer_token             text        not null,
    seller_token            text        not null,

    settlement_handle       text,
    session_id              uuid
);

create index deals_buyer_token_idx  on deals (buyer_token);
create index deals_seller_token_idx on deals (seller_token);
create index deals_state_idx        on deals (state);

create table witness_sessions (
    id          uuid primary key,
    deal_id     uuid        not null references deals (id) on delete cascade,
    transcript  jsonb       not null,
    started_at  timestamptz not null,
    closed      boolean     not null default false
);

create index witness_sessions_deal_idx on witness_sessions (deal_id);

create table attestations (
    id               uuid primary key,
    deal_id          uuid        not null references deals (id) on delete cascade,
    seq              integer     not null,
    action           text        not null,
    actor            jsonb       not null,
    at               timestamptz not null,
    payload          jsonb       not null,
    payload_hash     text        not null,
    prev_chain_hash  text        not null,
    chain_hash       text        not null,
    key_id           text        not null,
    signature        text,

    unique (deal_id, seq)
);

create index attestations_deal_seq_idx on attestations (deal_id, seq);

create table deal_events (
    id           bigserial primary key,
    deal_id      uuid        not null references deals (id) on delete cascade,
    seq          integer     not null,
    kind         text        not null,
    payload      jsonb       not null,
    occurred_at  timestamptz not null,

    unique (deal_id, seq)
);

-- Durable timers. Deadlines are the product, so they get a table rather than a
-- cron job scanning for work.
create table scheduled_tasks (
    id            uuid primary key,
    deal_id       uuid        not null references deals (id) on delete cascade,
    kind          text        not null,
    due_at        timestamptz not null,
    state         text        not null default 'pending',
    attempts      integer     not null default 0,
    locked_until  timestamptz,
    last_error    text,
    dedup_key     text        not null unique
);

create index scheduled_tasks_due_idx
    on scheduled_tasks (due_at)
    where state = 'pending';

-- The mock escrow ledger. Double-entry so that "the sum of every entry for a
-- handle is zero" is a checkable invariant rather than a hope; swapping in a
-- real PSP replaces these two tables and nothing else.
create table ledger_holds (
    handle       text primary key,
    deal_id      uuid        not null unique,
    currency     text        not null,
    minor_units  bigint      not null,
    state        text        not null,
    created_at   timestamptz not null,
    settled_at   timestamptz
);

create table ledger_entries (
    id           bigserial primary key,
    handle       text        not null references ledger_holds (handle) on delete cascade,
    account      text        not null,
    currency     text        not null,
    minor_units  bigint      not null,
    at           timestamptz not null,
    memo         text        not null
);

create index ledger_entries_handle_idx on ledger_entries (handle);

create table proof_objects (
    reference   text primary key,
    deal_id     uuid        not null references deals (id) on delete cascade,
    images      jsonb       not null,
    created_at  timestamptz not null
);
