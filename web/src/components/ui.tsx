import type { ReactNode } from "react";
import { formatMoney, STATE_LABEL, type DealState, type Offer, type Terms } from "../lib/api";

export function Page({ children }: { children: ReactNode }) {
  return (
    <div className="min-h-full">
      <div className="mx-auto w-full max-w-2xl px-5 py-8 pb-28">{children}</div>
    </div>
  );
}

export function Masthead({ sub }: { sub?: string }) {
  return (
    <header className="mb-8">
      <h1 className="font-display text-[2.1rem] leading-none tracking-tight">
        {/* The wordmark carries the brand gradient; nothing else does, so it
            stays a signature rather than a texture. */}
        <span className="bg-gradient-to-r from-cyan via-paper to-seal bg-clip-text text-transparent">
          True Handshake
        </span>
      </h1>
      {sub && <p className="mt-2 text-sm text-muted">{sub}</p>}
    </header>
  );
}

export function Card({
  children,
  title,
  className = "",
}: {
  children: ReactNode;
  title?: string;
  className?: string;
}) {
  return (
    <section
      className={`rise rounded-2xl border border-line bg-surface/80 p-5 backdrop-blur-sm ${className}`}
    >
      {title && (
        <h2 className="mb-4 font-display text-lg tracking-tight text-paper">{title}</h2>
      )}
      {children}
    </section>
  );
}

export function Button({
  children,
  onClick,
  disabled,
  variant = "primary",
  type = "button",
  full,
}: {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  variant?: "primary" | "cyan" | "ghost" | "danger";
  type?: "button" | "submit";
  full?: boolean;
}) {
  const base =
    "inline-flex items-center justify-center gap-2 rounded-xl px-5 py-3.5 text-sm font-semibold transition-all active:scale-[0.985] disabled:cursor-not-allowed disabled:opacity-40 disabled:active:scale-100";
  const styles = {
    // Cyan on the purple ground is 8.4:1; magenta on it is 2:1, so magenta
    // carries meaning as a fill and never as text.
    primary:
      "bg-seal text-paper shadow-[0_0_24px_-6px_var(--color-seal)] hover:brightness-110",
    cyan: "bg-magenta text-mint shadow-[0_0_24px_-6px_var(--color-magenta)] hover:brightness-110",
    ghost: "border border-line bg-raised/60 text-paper hover:border-cyan/60",
    danger: "bg-magenta text-mint hover:brightness-110",
  }[variant];

  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled}
      className={`${base} ${styles} ${full ? "w-full" : ""}`}
    >
      {children}
    </button>
  );
}

/** State is never conveyed by colour alone: every badge carries a word. */
export function StateBadge({ state }: { state: DealState }) {
  const mark =
    state === "completed" || state === "resolved"
      ? "✓"
      : state === "disputed" ||
          state === "cancelled" ||
          state === "expired" ||
          state === "refunded"
        ? "!"
        : "•";
  return (
    <span
      className={`state-${state} inline-flex items-center gap-2 rounded-full border border-line bg-raised/70 px-3.5 py-1.5 text-xs font-semibold tracking-wide`}
    >
      <span aria-hidden="true">{mark}</span>
      {STATE_LABEL[state]}
    </span>
  );
}

export function Hash({ value, label }: { value: string; label?: string }) {
  return (
    <span className="font-mono text-xs text-faint" title={value}>
      {label && <span className="mr-1">{label}</span>}
      {value.slice(0, 16)}…
    </span>
  );
}

export function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-line/60 py-2.5 last:border-0">
      <dt className="shrink-0 text-sm text-muted">{label}</dt>
      <dd className="text-right text-sm text-paper">{children}</dd>
    </div>
  );
}

export function Notice({
  tone = "info",
  title,
  children,
}: {
  tone?: "info" | "warn" | "good" | "bad";
  title?: string;
  children: ReactNode;
}) {
  const styles = {
    info: "border-cyan/25 bg-cyan/[0.06]",
    warn: "border-warn/30 bg-warn/[0.06]",
    good: "border-good/30 bg-good/[0.06]",
    bad: "border-bad/35 bg-bad/[0.07]",
  }[tone];
  const titleColor = {
    info: "text-cyan",
    warn: "text-warn",
    good: "text-good",
    bad: "text-bad",
  }[tone];

  return (
    <div className={`rise rounded-xl border p-4 text-sm ${styles}`} role="status">
      {title && <p className={`mb-1.5 font-semibold ${titleColor}`}>{title}</p>}
      <div className="text-paper/85">{children}</div>
    </div>
  );
}

/**
 * The receipt's payoff. Struck only when verification passed in the reader's
 * own browser — it is a result, not an ornament.
 */
export function Seal({ ok, label }: { ok: boolean; label: string }) {
  return (
    <div className="flex flex-col items-center py-2">
      <div
        className={`flex h-24 w-24 rotate-[-8deg] items-center justify-center rounded-full ${
          ok ? "seal-stamp" : "seal-stamp-bad"
        }`}
      >
        <span
          className={`text-center font-display text-[0.7rem] leading-tight tracking-widest uppercase ${
            ok ? "text-good" : "text-bad"
          }`}
        >
          {ok ? (
            <>
              veri
              <br />
              fied
            </>
          ) : (
            <>
              not
              <br />
              valid
            </>
          )}
        </span>
      </div>
      <p className={`mt-3 text-xs ${ok ? "text-good" : "text-bad"}`}>{label}</p>
    </div>
  );
}

/** A deadline, rendered large and calm. */
export function Countdown({ label, value, note }: { label: string; value: string; note?: string }) {
  return (
    <div className="text-center">
      <p className="text-sm text-muted">{label}</p>
      <p
        className="tabular mt-1 font-mono text-5xl font-light tracking-tight text-cyan"
        aria-live="polite"
      >
        {value}
      </p>
      {note && <p className="mt-2 text-xs text-muted">{note}</p>}
    </div>
  );
}

const KIND_LABEL: Record<Offer["kind"], string> = {
  context: "mentioned",
  ask: "asked",
  offer: "offered",
  counter: "countered",
  accept: "agreed",
};

/**
 * The negotiation ladder — the thing that makes this receipt worth more than a
 * checkbox. A spine connects the rungs so it reads as one sequence, and the
 * closing rung is the only one in mint.
 */
export function Ladder({ terms }: { terms: Terms }) {
  if (terms.ladder.length === 0) {
    return <p className="text-sm text-muted">No priced steps were captured.</p>;
  }
  return (
    <ol className="relative space-y-4">
      {/* the spine */}
      <span
        aria-hidden="true"
        className="absolute top-2 bottom-2 left-[3.5px] w-px bg-gradient-to-b from-seal/50 via-line to-good/50"
      />
      {terms.ladder.map((o) => {
        const who = o.by === "buyer" ? terms.buyer_name : terms.seller_name;
        const isClose = o.kind === "accept";
        return (
          <li key={o.seq} className="relative flex gap-4">
            <span
              aria-hidden="true"
              className={`relative z-10 mt-2 h-2 w-2 shrink-0 rounded-full ring-4 ring-surface ${
                isClose ? "bg-good" : o.kind === "context" ? "bg-line" : "bg-seal"
              }`}
            />
            <div className="min-w-0 flex-1">
              <p className="text-sm">
                <span className="font-semibold text-paper">{who}</span>{" "}
                <span className="text-muted">{KIND_LABEL[o.kind]}</span>
                {o.amount && (
                  <span
                    className={`ml-1.5 font-mono font-medium ${
                      isClose ? "text-good" : "text-paper"
                    }`}
                  >
                    {formatMoney(o.amount)}
                  </span>
                )}
              </p>
              <p className="mt-1 truncate text-xs text-faint italic">“{o.quote}”</p>
            </div>
          </li>
        );
      })}
    </ol>
  );
}

export function TermsSheet({ terms, hash }: { terms: Terms; hash?: string | null }) {
  return (
    <dl>
      <Field label="Item">
        {terms.item}
        {terms.item_detail ? ` — ${terms.item_detail}` : ""}
      </Field>
      {terms.condition && <Field label="Condition">{terms.condition}</Field>}
      <Field label="Price">
        <span className="font-mono text-lg font-medium text-cyan">
          {formatMoney(terms.price)}
        </span>
      </Field>
      <Field label="Buyer">{terms.buyer_name}</Field>
      <Field label="Seller">{terms.seller_name}</Field>
      <Field label="Handoff">{terms.handoff.replace("_", " ")}</Field>
      <Field label="Settlement">{terms.settlement.kind.replace("_", " ")}</Field>
      {hash && (
        <Field label="Terms hash">
          <Hash value={hash} />
        </Field>
      )}
    </dl>
  );
}

export function Spinner({ label = "Working…" }: { label?: string }) {
  return (
    <p className="flex items-center justify-center gap-2.5 py-2 text-sm text-muted" role="status">
      <span className="pulse-dot h-2 w-2 rounded-full bg-cyan" aria-hidden="true" />
      {label}
    </p>
  );
}
