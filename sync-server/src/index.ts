/**
 * MyTube's sync server. Each sync group is one SQLite-backed Durable Object holding up to five
 * opaque, client-encrypted blobs ("slots"), one per machine. The Worker in front validates every
 * request before any Durable Object is reached, because the account's free quota (100k Worker
 * requests, 100k DO requests and 100k rows written a day) is shared with other projects: a
 * malformed or hostile request should cost one Worker invocation and nothing else.
 *
 * The contract (paths, bodies, status codes, limits) is section 1 "Endpoints" and section 2 of
 * the cloud sync design spec. The Rust client is written against the same text.
 */
import { DurableObject } from "cloudflare:workers";

declare global {
  namespace Cloudflare {
    interface Env {
      GROUPS: DurableObjectNamespace<SyncGroup>;
      /** Public. From wrangler.jsonc `vars` (or .dev.vars locally); empty means not configured. */
      TURNSTILE_SITE_KEY: string;
      /** `wrangler secret put TURNSTILE_SECRET`; never in the repository. */
      TURNSTILE_SECRET?: string;
    }
  }
}
type Env = Cloudflare.Env;

// ---------------------------------------------------------------------------------------------
// Limits (spec section 2)
// ---------------------------------------------------------------------------------------------

/** Leaves ~21 KB over the largest blob for the JSON around it (machine id, `have`, the token). */
const MAX_BODY_BYTES = 720_000;
/** 512 KiB of raw blob as base64url without padding: ceil(524288 * 4 / 3). */
const MAX_BLOB_CHARS = 699_051;
/** Machines per group, and so also the most entries `have` can sensibly carry. */
const MAX_SLOTS = 5;
/** Real Turnstile tokens are well under this; it only bounds what we forward to siteverify. */
const MAX_TOKEN_CHARS = 2048;
/** A slot, and the whole group, goes after this long without a write. */
const EXPIRY_SECONDS = 90 * 24 * 60 * 60;

const BEARER = /^Bearer ([A-Za-z0-9_-]{43})$/;
const MACHINE = /^[A-Za-z0-9_-]{22}$/;
const BLOB = /^[A-Za-z0-9_-]+$/;

const SITEVERIFY_URL = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

// ---------------------------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------------------------

export default {
  async fetch(request, env): Promise<Response> {
    const route = `${request.method} ${new URL(request.url).pathname}`;
    try {
      switch (route) {
        case "POST /v1/group":
          return await createGroup(request, env);
        case "POST /v1/sync":
          return await syncGroup(request, env);
        case "GET /verify":
          return verifyPage(env.TURNSTILE_SITE_KEY);
        default:
          return jsonError(404, "not_found");
      }
    } catch (e) {
      if (e instanceof Refusal) return jsonError(e.status, e.error);
      throw e;
    }
  },
} satisfies ExportedHandler<Env>;

/**
 * A request turned away with one of the contract's error bodies. Thrown by the validators so each
 * handler reads as a straight line; caught once, in `fetch`.
 */
class Refusal {
  constructor(
    readonly status: number,
    readonly error: string,
  ) {}
}
const BAD_REQUEST = new Refusal(400, "bad_request");
const TOO_LARGE = new Refusal(413, "too_large");

async function createGroup(request: Request, env: Env): Promise<Response> {
  const bearer = bearerOf(request);
  const body = await readBody(request);
  const machine = machineOf(body.machine);
  const blob = blobOf(body.blob);
  const token = turnstileTokenOf(body.turnstileToken);

  // A deployment without its secret can't verify anyone. Saying so beats a misleading 403, and it
  // skips a siteverify call that could only fail.
  if (!env.TURNSTILE_SECRET) throw new Refusal(503, "not_configured");
  // Verified before the Durable Object is touched: creating a group is the one request that
  // writes storage for a bearer nobody has used before.
  if (!(await passesTurnstile(env.TURNSTILE_SECRET, token, request.headers.get("CF-Connecting-IP")))) {
    throw new Refusal(403, "verification_failed");
  }

  const created = await (await groupFor(env, bearer)).create(machine, blob);
  return created ? json(201, { rev: 1 }) : jsonError(409, "exists");
}

async function syncGroup(request: Request, env: Env): Promise<Response> {
  const bearer = bearerOf(request);
  const body = await readBody(request);
  const machine = machineOf(body.machine);
  const have = haveOf(body.have);
  // `"blob"?: string` in the contract. A null is read as absent too, since that's how a
  // serialiser writes an empty optional unless told to skip it.
  const blob = body.blob === undefined || body.blob === null ? null : blobOf(body.blob);

  const outcome = await (await groupFor(env, bearer)).sync(machine, have, blob);
  if (!outcome.ok) return jsonError(outcome.error === "no_group" ? 404 : 409, outcome.error);
  // The object already serialised the body, so blobs pass through the Worker as one string,
  // never parsed or re-stringified here: the Worker is the half with the 10 ms CPU budget.
  return new Response(outcome.body, { status: 200, headers: JSON_HEADERS });
}

/**
 * The group's object is named by hex(SHA-256(bearer)), so what Cloudflare stores as the name
 * can't be replayed as a bearer, and groups can't be listed back into codes.
 */
async function groupFor(env: Env, bearer: string): Promise<DurableObjectStub<SyncGroup>> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(bearer));
  const hex = Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
  return env.GROUPS.get(env.GROUPS.idFromName(hex));
}

/** Any failure to get a clear `success: true` (network, non-2xx, odd JSON) fails closed. */
async function passesTurnstile(secret: string, token: string, ip: string | null): Promise<boolean> {
  const form = new URLSearchParams({ secret, response: token });
  if (ip) form.set("remoteip", ip);
  try {
    const res = await fetch(SITEVERIFY_URL, { method: "POST", body: form });
    if (!res.ok) return false;
    const outcome = await res.json<{ success?: unknown }>();
    return outcome.success === true;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------------------------
// Validation. Every check here runs before any Durable Object is addressed.
// ---------------------------------------------------------------------------------------------

function bearerOf(request: Request): string {
  const match = BEARER.exec(request.headers.get("Authorization") ?? "");
  if (!match) throw BAD_REQUEST;
  return match[1];
}

/**
 * Reads the body as a JSON object, never holding more than MAX_BODY_BYTES of it. A declared
 * Content-Length over the cap is refused unread; a chunked body is counted as it streams in and
 * cut off the moment it passes the cap, so an endless upload costs a fixed amount of memory.
 */
async function readBody(request: Request): Promise<Record<string, unknown>> {
  if (Number(request.headers.get("Content-Length")) > MAX_BODY_BYTES) throw TOO_LARGE;
  if (!request.body) throw BAD_REQUEST;

  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > MAX_BODY_BYTES) {
      await reader.cancel();
      throw TOO_LARGE;
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.byteLength;
  }

  let value: unknown;
  try {
    // `fatal` so invalid UTF-8 is a bad request rather than silently becoming U+FFFD.
    value = JSON.parse(new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(bytes));
  } catch {
    // Not logged: a JSON.parse message quotes the input, which may be a blob.
    throw BAD_REQUEST;
  }
  if (!isObject(value)) throw BAD_REQUEST;
  return value;
}

function machineOf(value: unknown): string {
  if (typeof value !== "string" || !MACHINE.test(value)) throw BAD_REQUEST;
  return value;
}

function blobOf(value: unknown): string {
  // Length first, so an oversized blob is never scanned.
  if (typeof value !== "string" || value.length > MAX_BLOB_CHARS || !BLOB.test(value)) {
    throw BAD_REQUEST;
  }
  return value;
}

function turnstileTokenOf(value: unknown): string {
  if (typeof value !== "string" || value.length === 0 || value.length > MAX_TOKEN_CHARS) {
    throw BAD_REQUEST;
  }
  return value;
}

/** `{machineId: rev}`, at most five entries, each rev a non-negative safe integer. */
function haveOf(value: unknown): Record<string, number> {
  if (!isObject(value)) throw BAD_REQUEST;
  const entries = Object.entries(value);
  if (entries.length > MAX_SLOTS) throw BAD_REQUEST;
  const have: Record<string, number> = {};
  for (const [machine, rev] of entries) {
    if (!MACHINE.test(machine) || typeof rev !== "number" || !Number.isSafeInteger(rev) || rev < 0) {
      throw BAD_REQUEST;
    }
    have[machine] = rev;
  }
  return have;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// ---------------------------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------------------------

const JSON_HEADERS = { "Content-Type": "application/json", "Cache-Control": "no-store" };

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: JSON_HEADERS });
}

function jsonError(status: number, error: string): Response {
  return json(status, { error });
}

// ---------------------------------------------------------------------------------------------
// The sync group
// ---------------------------------------------------------------------------------------------

export type SyncOutcome =
  | { ok: true; body: string }
  | { ok: false; error: "no_group" | "full" };

interface SlotHead extends Record<string, SqlStorageValue> {
  machine: string;
  rev: number;
  written_at: number;
}

export class SyncGroup extends DurableObject<Env> {
  /**
   * Creates the group with `machine`'s slot at rev 1, or answers false if it already exists.
   * This is the only method that creates tables, so a request for a group nobody created
   * writes nothing.
   */
  async create(machine: string, blob: string): Promise<boolean> {
    if (this.#exists()) return false;
    const sql = this.ctx.storage.sql;
    this.ctx.storage.transactionSync(() => {
      sql.exec("CREATE TABLE meta (k TEXT PRIMARY KEY, v INTEGER)");
      sql.exec(
        `CREATE TABLE slots (
           machine TEXT PRIMARY KEY,
           rev INTEGER NOT NULL,
           blob TEXT NOT NULL,
           written_at INTEGER NOT NULL
         )`,
      );
      sql.exec("INSERT INTO meta (k, v) VALUES ('rev', 1)");
      sql.exec(
        "INSERT INTO slots (machine, rev, blob, written_at) VALUES (?, 1, ?, ?)",
        machine,
        blob,
        unixNow(),
      );
    });
    await this.#extendLife();
    return true;
  }

  /**
   * One round trip: store `blob` as this machine's slot when given, then hand back every other
   * slot newer than what the caller already has. Even a read-only call from a sixth machine is
   * refused, so a full group never leaks its slots to a machine that can't join it.
   */
  async sync(machine: string, have: Record<string, number>, blob: string | null): Promise<SyncOutcome> {
    if (!this.#exists()) return { ok: false, error: "no_group" };
    const sql = this.ctx.storage.sql;
    const now = unixNow();

    // A machine silent for 90 days has left in all but name; dropping it frees its place.
    sql.exec("DELETE FROM slots WHERE written_at < ?", now - EXPIRY_SECONDS);

    const known = sql.exec("SELECT 1 FROM slots WHERE machine = ?", machine).toArray().length > 0;
    if (!known && sql.exec<{ n: number }>("SELECT COUNT(*) AS n FROM slots").one().n >= MAX_SLOTS) {
      return { ok: false, error: "full" };
    }

    let rev = sql.exec<{ v: number }>("SELECT v FROM meta WHERE k = 'rev'").one().v;
    if (blob !== null) {
      rev += 1;
      this.ctx.storage.transactionSync(() => {
        sql.exec("UPDATE meta SET v = ? WHERE k = 'rev'", rev);
        sql.exec(
          `INSERT INTO slots (machine, rev, blob, written_at) VALUES (?, ?, ?, ?)
           ON CONFLICT (machine) DO UPDATE
           SET rev = excluded.rev, blob = excluded.blob, written_at = excluded.written_at`,
          machine,
          rev,
          blob,
          now,
        );
      });
      await this.#extendLife();
    }

    const heads = sql
      .exec<SlotHead>("SELECT machine, rev, written_at FROM slots ORDER BY machine")
      .toArray();
    // Blobs are read only for the slots actually going out: the heads decide, then each wanted
    // blob is one more row read rather than every blob in the group.
    const slots = heads
      .filter((s) => s.machine !== machine && s.rev > (Object.hasOwn(have, s.machine) ? have[s.machine] : 0))
      .map((s) => ({
        machine: s.machine,
        rev: s.rev,
        written_at: s.written_at,
        blob: sql.exec<{ blob: string }>("SELECT blob FROM slots WHERE machine = ?", s.machine).one().blob,
      }));

    return { ok: true, body: JSON.stringify({ rev, machines: heads.map((s) => s.machine), slots }) };
  }

  /** 90 days without a write: the group goes, tables and all, and its code answers no_group. */
  override async alarm(): Promise<void> {
    await this.ctx.storage.deleteAlarm();
    await this.ctx.storage.deleteAll();
  }

  /** Read-only: sqlite_master is consulted, never written, so probing an unknown group is free. */
  #exists(): boolean {
    return (
      this.ctx.storage.sql
        .exec("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'")
        .toArray().length > 0
    );
  }

  /** Every write pushes the group's expiry 90 days out. */
  async #extendLife(): Promise<void> {
    await this.ctx.storage.setAlarm(Date.now() + EXPIRY_SECONDS * 1000);
  }
}

function unixNow(): number {
  return Math.floor(Date.now() / 1000);
}

// ---------------------------------------------------------------------------------------------
// GET /verify
// ---------------------------------------------------------------------------------------------

/**
 * The Turnstile page MyTube opens in its own window. On success it navigates to
 * /verify/done?token=…, which the app intercepts and cancels, so that path never has to exist
 * here (and the edge's custom rule would block it if it ever escaped).
 */
function verifyPage(siteKey: string): Response {
  const headers = { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" };
  if (!siteKey) {
    return new Response(
      page(
        "MyTube sync is unavailable",
        `<h1>Sync isn't available</h1>
         <p>This sync server isn't configured yet, so it can't create sync groups.</p>`,
      ),
      { status: 503, headers },
    );
  }
  return new Response(
    page(
      "Turn on MyTube sync",
      `<h1>Turn on MyTube sync</h1>
       <p>Confirm you're a person. MyTube then creates your sync group.</p>
       <div id="widget" data-sitekey="${escapeHtml(siteKey)}"></div>
       <p id="status" role="status" aria-live="polite"></p>
       <noscript><p class="error">This check needs JavaScript.</p></noscript>
       <script>
         // The site key travels in a data attribute, so HTML escaping is the only escaping it needs.
         function onTurnstileLoad() {
           const box = document.getElementById("widget");
           const status = document.getElementById("status");
           const say = (text, isError) => {
             status.textContent = text;
             status.className = isError ? "error" : "";
           };
           let failures = 0;
           // Turnstile's own retries are off so these handlers are the only ones resetting the
           // widget; a broken widget then can't spin in a loop of challenge requests.
           const widget = turnstile.render(box, {
             sitekey: box.dataset.sitekey,
             retry: "never",
             "refresh-expired": "manual",
             callback(token) {
               say("Verified. Returning to MyTube…", false);
               location.assign("/verify/done?token=" + encodeURIComponent(token));
             },
             "error-callback"() {
               failures += 1;
               if (failures >= 3) {
                 say("The check keeps failing. Close this window and try again.", true);
               } else {
                 say("The check didn't go through. Trying again…", true);
                 setTimeout(() => turnstile.reset(widget), 1500);
               }
               return true;
             },
             "expired-callback"() {
               say("The check expired. Trying again…", true);
               turnstile.reset(widget);
             },
           });
         }
       </script>
       <script src="https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit&amp;onload=onTurnstileLoad" async defer></script>`,
    ),
    { status: 200, headers },
  );
}

function page(title: string, content: string): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>${title}</title>
<style>
  :root {
    color-scheme: light dark;
    --bg: #f4f4f4; --surface: #ffffff; --line: #e1e1e1;
    --text: #0f0f0f; --muted: #5c5c5c; --danger: #c4282d;
  }
  @media (prefers-color-scheme: dark) {
    :root {
      --bg: #0f0f0f; --surface: #181818; --line: #303030;
      --text: #f1f1f1; --muted: #aaaaaa; --danger: #ff5a5f;
    }
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; min-height: 100vh; padding: 24px 16px;
    display: grid; place-items: center;
    background: var(--bg); color: var(--text);
    font: 15px/1.5 system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
    -webkit-font-smoothing: antialiased;
  }
  main {
    width: 100%; max-width: 380px; padding: 28px 24px;
    background: var(--surface); border: 1px solid var(--line); border-radius: 12px;
    text-align: center;
  }
  h1 { margin: 0 0 8px; font-size: 20px; font-weight: 600; letter-spacing: -0.01em; }
  p { margin: 0; color: var(--muted); }
  #widget { min-height: 65px; margin: 20px 0 12px; display: flex; justify-content: center; }
  #status { min-height: 1.5em; font-size: 13px; }
  .error, #status.error { color: var(--danger); }
</style>
</head>
<body>
<main>
${content}
</main>
</body>
</html>
`;
}

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}
