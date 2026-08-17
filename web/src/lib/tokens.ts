/**
 * Per-deal bearer tokens, kept in localStorage — one slot per role.
 *
 * Two people around one phone is the normal case for this product: they are
 * standing together, talking into a single microphone, and asking the second
 * one to open a private window is absurd. So a device can hold both sides of a
 * deal and switch between them.
 *
 * An earlier version stored a single token per deal, which meant opening the
 * counterpart link silently overwrote the first party's credentials and a reload
 * flipped your identity.
 *
 * None of this is authentication: whoever holds the link is that party, and a
 * device holding both can act as both. That is why a confirmation made this way
 * is reported to the server as `same_device` and labelled on the receipt, rather
 * than quietly passed off as two independent ones.
 */

export type Role = "buyer" | "seller";

const tokenKey = (dealId: string, role: Role) => `th:token:${dealId}:${role}`;
const activeKey = (dealId: string) => `th:active:${dealId}`;
const counterpartKey = (dealId: string) => `th:counterpart:${dealId}`;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private browsing with storage disabled. The URL token still works for
    // this navigation; we just cannot remember it.
  }
}

export function storeToken(dealId: string, role: Role, token: string) {
  write(tokenKey(dealId, role), token);
  write(activeKey(dealId), role);
}

export function getToken(dealId: string, role: Role): string | null {
  return read(tokenKey(dealId, role));
}

/** Which sides of this deal this device can speak for. */
export function heldRoles(dealId: string): Role[] {
  return (["buyer", "seller"] as Role[]).filter((r) => getToken(dealId, r));
}

export function getActiveRole(dealId: string): Role | null {
  const stored = read(activeKey(dealId)) as Role | null;
  if (stored && getToken(dealId, stored)) return stored;
  return heldRoles(dealId)[0] ?? null;
}

export function setActiveRole(dealId: string, role: Role) {
  write(activeKey(dealId), role);
}

/**
 * The token to use on this page load: whatever the URL carries wins, since
 * following a fresh link is an explicit statement of which party you are.
 * Its role is unknown until the server says, so it is filed afterwards.
 */
export function incomingToken(search: string): string | null {
  return new URLSearchParams(search).get("t");
}

/** The other party's full link, so it can be handed over when they leave. */
export function rememberCounterpartLink(dealId: string, url: string) {
  write(counterpartKey(dealId), url);
}

export function recallCounterpartLink(dealId: string): string | null {
  return read(counterpartKey(dealId));
}
