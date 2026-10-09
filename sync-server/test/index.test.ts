// Runs inside workerd: the real Worker, a real SQLite-backed SyncGroup object, and the real
// request path through `exports.default`. Only Turnstile's siteverify is mocked, by spying on the
// global fetch the Worker shares with these tests.
import { runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { env, exports } from "cloudflare:workers";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import worker from "../src/index";

const ORIGIN = "https://mytube-sync.blyat.uk";
const SITEVERIFY = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const TOKEN = "XXXX.DUMMY.TOKEN.XXXX";
const DAY = 24 * 60 * 60;
const MAX_BODY_BYTES = 720_000;
const MAX_BLOB_CHARS = 699_051;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

function base64url(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
/** A fresh 43-char bearer per test, so no test sees another's group. */
const newBearer = () => base64url(crypto.getRandomValues(new Uint8Array(32)));
/** A fresh 22-char machine id. */
const newMachine = () => base64url(crypto.getRandomValues(new Uint8Array(16)));
const unixNow = () => Math.floor(Date.now() / 1000);

function post(path: string, bearer: string | null, body: unknown, headers: Record<string, string> = {}) {
  return exports.default.fetch(`${ORIGIN}${path}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      ...(bearer === null ? {} : { Authorization: `Bearer ${bearer}` }),
      ...headers,
    },
    body: typeof body === "string" || body instanceof Uint8Array ? body : JSON.stringify(body),
  });
}

function createGroup(bearer: string, machine: string, blob = "c2xvdC1h", headers: Record<string, string> = {}) {
  return post("/v1/group", bearer, { turnstileToken: TOKEN, machine, blob }, headers);
}

function sync(bearer: string, machine: string, have: Record<string, number> = {}, blob?: string) {
  return post("/v1/sync", bearer, blob === undefined ? { machine, have } : { machine, have, blob });
}

interface SyncBody {
  rev: number;
  machines: string[];
  slots: { machine: string; rev: number; written_at: number; blob: string }[];
}

async function syncOk(bearer: string, machine: string, have: Record<string, number> = {}, blob?: string) {
  const res = await sync(bearer, machine, have, blob);
  expect(res.status).toBe(200);
  return (await res.json()) as SyncBody;
}

/** The group's object, addressed independently of the Worker: hex(SHA-256(bearer)). */
async function groupOf(bearer: string) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(bearer));
  const hex = Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
  return env.GROUPS.get(env.GROUPS.idFromName(hex));
}

/** What the group's object has stored: its tables, its alarm and the SQLite file's size. */
async function storageOf(bearer: string) {
  return runInDurableObject(await groupOf(bearer), async (_, state) => ({
    tables: state.storage.sql
      .exec<{ name: string }>("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
      .toArray()
      .map((r) => r.name),
    alarm: await state.storage.getAlarm(),
    size: state.storage.sql.databaseSize,
  }));
}

/** An object nobody has ever sent a request to, for comparison. */
async function untouchedSize() {
  const stub = env.GROUPS.get(env.GROUPS.newUniqueId());
  return runInDurableObject(stub, (_, state) => state.storage.sql.databaseSize);
}

async function expectNoStorage(bearer: string) {
  const stored = await storageOf(bearer);
  expect(stored.tables).toEqual([]);
  expect(stored.alarm).toBeNull();
  expect(stored.size).toBe(await untouchedSize());
}

async function expectError(res: Response, status: number, error: string) {
  expect(res.status).toBe(status);
  expect(res.headers.get("Content-Type")).toBe("application/json");
  expect(res.headers.get("Cache-Control")).toBe("no-store");
  expect(await res.json()).toEqual({ error });
}

/** The forms the Worker posted to siteverify during this test. */
let siteverifyCalls: Record<string, string>[] = [];

function answerSiteverify(answer: () => Response | Promise<Response>) {
  siteverifyCalls = [];
  vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const request = new Request(input, init);
    if (request.url !== SITEVERIFY) throw new Error(`unexpected outbound fetch: ${request.url}`);
    const form = await request.formData();
    siteverifyCalls.push(Object.fromEntries([...form].map(([k, v]) => [k, String(v)])));
    return answer();
  });
}

/** Calls the handler directly, for the env overrides `exports.default` can't take. */
function fetchWithEnv(request: Request, overrides: Partial<Cloudflare.Env>) {
  return worker.fetch(request as Parameters<typeof worker.fetch>[0], { ...env, ...overrides });
}

beforeEach(() => answerSiteverify(() => Response.json({ success: true, "error-codes": [] })));
afterEach(() => vi.restoreAllMocks());

// ---------------------------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------------------------

describe("routing", () => {
  it.each([
    ["GET", "/"],
    ["GET", "/v1/sync"],
    ["GET", "/v1/group"],
    ["PUT", "/v1/group"],
    ["DELETE", "/v1/sync"],
    ["POST", "/v1/sync/"],
    ["POST", "/v1/other"],
    ["POST", "/v2/sync"],
    ["POST", "/verify"],
    ["GET", "/verify/done?token=abc"],
    ["GET", "/favicon.ico"],
  ])("%s %s is 404 not_found", async (method, path) => {
    const res = await exports.default.fetch(`${ORIGIN}${path}`, {
      method,
      headers: { Authorization: `Bearer ${newBearer()}` },
      body: method === "GET" ? undefined : "{}",
    });
    await expectError(res, 404, "not_found");
  });
});

// ---------------------------------------------------------------------------------------------
// Validation: every rejection happens before storage, and before siteverify
// ---------------------------------------------------------------------------------------------

describe("validation", () => {
  const good = {
    group: () => ({ turnstileToken: TOKEN, machine: newMachine(), blob: "AAAA" }),
    sync: () => ({ machine: newMachine(), have: {} }),
  };

  describe.each(["/v1/group", "/v1/sync"] as const)("%s", (path) => {
    const body = () => (path === "/v1/group" ? good.group() : good.sync());

    it.each([
      ["missing", null],
      ["42 chars", "Bearer " + "A".repeat(42)],
      ["44 chars", "Bearer " + "A".repeat(44)],
      ["a '+'", "Bearer " + "A".repeat(42) + "+"],
      ["padding", "Bearer " + "A".repeat(42) + "="],
      ["a lowercase scheme", "bearer " + "A".repeat(43)],
      ["another scheme", "Basic " + "A".repeat(43)],
      ["two spaces", "Bearer  " + "A".repeat(43)],
      ["trailing text", "Bearer " + "A".repeat(43) + " x"],
    ])("refuses a bearer with %s", async (_, authorization) => {
      const res = await exports.default.fetch(`${ORIGIN}${path}`, {
        method: "POST",
        headers: authorization === null ? {} : { Authorization: authorization },
        body: JSON.stringify(body()),
      });
      await expectError(res, 400, "bad_request");
      expect(siteverifyCalls).toHaveLength(0);
    });

    it.each([
      ["empty", ""],
      ["not JSON", "machine=x"],
      ["truncated JSON", '{"machine":'],
      ["an array", "[]"],
      ["null", "null"],
      ["a string", '"x"'],
      ["a number", "1"],
    ])("refuses a body that is %s", async (_, raw) => {
      const bearer = newBearer();
      await expectError(await post(path, bearer, raw), 400, "bad_request");
      expect(siteverifyCalls).toHaveLength(0);
      await expectNoStorage(bearer);
    });

    it("refuses a body that is not valid UTF-8", async () => {
      const json = new TextEncoder().encode(JSON.stringify(body()));
      const broken = new Uint8Array([...json.slice(0, -1), 0xff, 0x7d]); // ...\xff}
      await expectError(await post(path, newBearer(), broken), 400, "bad_request");
    });

    it.each([
      ["missing", undefined],
      ["21 chars", "A".repeat(21)],
      ["23 chars", "A".repeat(23)],
      ["a '='", "A".repeat(21) + "="],
      ["a '/'", "A".repeat(21) + "/"],
      ["a number", 1234],
      ["null", null],
    ])("refuses a machine that is %s", async (_, machine) => {
      const bearer = newBearer();
      await expectError(await post(path, bearer, { ...body(), machine }), 400, "bad_request");
      expect(siteverifyCalls).toHaveLength(0);
      await expectNoStorage(bearer);
    });

    it.each([
      ["empty", ""],
      ["padded", "AAA="],
      ["standard base64", "AA+/"],
      ["spaced", "AA AA"],
      ["a number", 42],
      ["an object", { a: 1 }],
      ["one char too long", "A".repeat(MAX_BLOB_CHARS + 1)],
    ])("refuses a blob that is %s", async (_, blob) => {
      const bearer = newBearer();
      await expectError(await post(path, bearer, { ...body(), blob }), 400, "bad_request");
      expect(siteverifyCalls).toHaveLength(0);
      await expectNoStorage(bearer);
    });

    it("refuses a declared Content-Length over 720,000 bytes", async () => {
      const bearer = newBearer();
      const res = await post(path, bearer, " ".repeat(MAX_BODY_BYTES + 1));
      await expectError(res, 413, "too_large");
      await expectNoStorage(bearer);
    });

    it("cuts off a streamed body at 720,000 bytes instead of buffering it", async () => {
      // An endless body with no Content-Length: the Worker has to stop reading on its own,
      // or this test never finishes. workerd prints "read end of pipe was aborted" for it;
      // that is the cut-off working, not a failure.
      const chunk = new TextEncoder().encode(" ".repeat(64 * 1024));
      const endless = new ReadableStream<Uint8Array>({
        pull(controller) {
          controller.enqueue(chunk);
        },
      });
      const res = await exports.default.fetch(`${ORIGIN}${path}`, {
        method: "POST",
        headers: { Authorization: `Bearer ${newBearer()}`, "Content-Type": "application/json" },
        body: endless,
      });
      await expectError(res, 413, "too_large");
    });

    it("takes a body of exactly 720,000 bytes", async () => {
      const bearer = newBearer();
      const json = JSON.stringify(body());
      const padded = json.slice(0, -1) + " ".repeat(MAX_BODY_BYTES - json.length) + "}";
      expect(new TextEncoder().encode(padded).byteLength).toBe(MAX_BODY_BYTES);
      const res = await post(path, bearer, padded);
      // Past validation: a new group, or one that doesn't exist yet.
      expect(res.status).toBe(path === "/v1/group" ? 201 : 404);
    });
  });

  describe("/v1/group", () => {
    it.each([
      ["missing", undefined],
      ["empty", ""],
      ["2049 chars", "x".repeat(2049)],
      ["a number", 7],
      ["null", null],
    ])("refuses a turnstileToken that is %s", async (_, turnstileToken) => {
      const bearer = newBearer();
      const res = await post("/v1/group", bearer, { ...good.group(), turnstileToken });
      await expectError(res, 400, "bad_request");
      expect(siteverifyCalls).toHaveLength(0);
      await expectNoStorage(bearer);
    });

    it("takes a turnstileToken of 2048 chars", async () => {
      const res = await post("/v1/group", newBearer(), { ...good.group(), turnstileToken: "x".repeat(2048) });
      expect(res.status).toBe(201);
    });

    it("needs a blob", async () => {
      const bearer = newBearer();
      const { blob: _, ...noBlob } = good.group();
      await expectError(await post("/v1/group", bearer, noBlob), 400, "bad_request");
      await expectError(await post("/v1/group", bearer, { ...noBlob, blob: null }), 400, "bad_request");
      expect(siteverifyCalls).toHaveLength(0);
    });
  });

  describe("/v1/sync", () => {
    const m = () => newMachine();

    it.each([
      ["missing", () => undefined],
      ["null", () => null],
      ["an array", () => [[m(), 1]]],
      ["a string", () => "{}"],
      ["six entries", () => Object.fromEntries(Array.from({ length: 6 }, () => [m(), 1]))],
      ["a bad key", () => ({ ["A".repeat(21)]: 1 })],
      ["a negative rev", () => ({ [m()]: -1 })],
      ["a fractional rev", () => ({ [m()]: 1.5 })],
      ["a string rev", () => ({ [m()]: "1" })],
      ["a null rev", () => ({ [m()]: null })],
      ["an unsafe rev", () => ({ [m()]: 2 ** 53 })],
    ])("refuses a have that is %s", async (_, have) => {
      const bearer = newBearer();
      await createGroup(bearer, m());
      const before = await storageOf(bearer);
      const res = await post("/v1/sync", bearer, { machine: m(), have: have(), blob: "AAAA" });
      await expectError(res, 400, "bad_request");
      // Nothing was written: the existing group is exactly as it was.
      expect(await storageOf(bearer)).toEqual(before);
    });

    it("takes a have of five entries, including the caller", async () => {
      const bearer = newBearer();
      const me = m();
      await createGroup(bearer, me);
      const have = Object.fromEntries([me, m(), m(), m(), m()].map((id) => [id, Number.MAX_SAFE_INTEGER]));
      expect((await syncOk(bearer, me, have)).slots).toEqual([]);
    });
  });
});

// ---------------------------------------------------------------------------------------------
// POST /v1/group
// ---------------------------------------------------------------------------------------------

describe("POST /v1/group", () => {
  it("verifies the token, then creates the group with this machine at rev 1", async () => {
    const bearer = newBearer();
    const machine = newMachine();
    const res = await createGroup(bearer, machine, "Zmlyc3Q", { "CF-Connecting-IP": "203.0.113.7" });

    expect(res.status).toBe(201);
    expect(res.headers.get("Content-Type")).toBe("application/json");
    expect(res.headers.get("Cache-Control")).toBe("no-store");
    expect(await res.json()).toEqual({ rev: 1 });

    expect(siteverifyCalls).toHaveLength(1);
    expect(siteverifyCalls[0]).toEqual({
      secret: "1x0000000000000000000000000000000AA",
      response: TOKEN,
      remoteip: "203.0.113.7",
    });

    // workerd keeps the alarm in its own _cf_ table beside ours.
    const stored = await storageOf(bearer);
    expect(stored.tables.filter((t) => !t.startsWith("_cf_"))).toEqual(["meta", "slots"]);
    const rows = await runInDurableObject(await groupOf(bearer), (_, state) => ({
      meta: state.storage.sql.exec("SELECT k, v FROM meta").toArray(),
      slots: state.storage.sql.exec("SELECT machine, rev, blob FROM slots").toArray(),
    }));
    expect(rows).toEqual({ meta: [{ k: "rev", v: 1 }], slots: [{ machine, rev: 1, blob: "Zmlyc3Q" }] });
  });

  it("leaves remoteip out when the request carries no CF-Connecting-IP", async () => {
    expect((await createGroup(newBearer(), newMachine())).status).toBe(201);
    expect(siteverifyCalls[0]).not.toHaveProperty("remoteip");
  });

  it("answers 409 exists for a group that is already there, and changes nothing", async () => {
    const bearer = newBearer();
    const first = newMachine();
    expect((await createGroup(bearer, first, "Zmlyc3Q")).status).toBe(201);
    await expectError(await createGroup(bearer, newMachine(), "c2Vjb25k"), 409, "exists");

    const body = await syncOk(bearer, newMachine());
    expect(body.rev).toBe(1);
    expect(body.machines).toEqual([first]);
    expect(body.slots.map((s) => s.blob)).toEqual(["Zmlyc3Q"]);
  });

  it("takes a blob of exactly 699,051 chars", async () => {
    const bearer = newBearer();
    const blob = "A".repeat(MAX_BLOB_CHARS);
    expect((await createGroup(bearer, newMachine(), blob)).status).toBe(201);
    expect((await syncOk(bearer, newMachine())).slots[0].blob).toBe(blob);
  });

  it.each([
    ["success: false", () => Response.json({ success: false, "error-codes": ["invalid-input-response"] })],
    ["success: \"true\"", () => Response.json({ success: "true" })],
    ["a 500", () => new Response("oops", { status: 500 })],
    ["a non-JSON answer", () => new Response("<html>")],
    ["a network failure", () => Promise.reject(new TypeError("Network connection lost."))],
  ])("answers 403 verification_failed on %s, and stores nothing", async (_, answer) => {
    answerSiteverify(answer);
    const bearer = newBearer();
    await expectError(await createGroup(bearer, newMachine()), 403, "verification_failed");
    expect(siteverifyCalls).toHaveLength(1);
    await expectNoStorage(bearer);
  });

  it("answers 503 not_configured without a TURNSTILE_SECRET, and stores nothing", async () => {
    const bearer = newBearer();
    const request = new Request(`${ORIGIN}/v1/group`, {
      method: "POST",
      headers: { Authorization: `Bearer ${bearer}` },
      body: JSON.stringify({ turnstileToken: TOKEN, machine: newMachine(), blob: "AAAA" }),
    });
    const res = await fetchWithEnv(request, { TURNSTILE_SECRET: "" });
    await expectError(res, 503, "not_configured");
    expect(siteverifyCalls).toHaveLength(0);
    await expectNoStorage(bearer);
  });
});

// ---------------------------------------------------------------------------------------------
// POST /v1/sync
// ---------------------------------------------------------------------------------------------

describe("POST /v1/sync", () => {
  it("answers 404 no_group for a bearer nobody created, and leaves no storage", async () => {
    const bearer = newBearer();
    await expectError(await sync(bearer, newMachine()), 404, "no_group");
    await expectError(await sync(bearer, newMachine(), {}, "AAAA"), 404, "no_group");
    await expectNoStorage(bearer);
  });

  it("round-trips slots between machines, filtered by have", async () => {
    const bearer = newBearer();
    const [a, b, c] = [newMachine(), newMachine(), newMachine()];
    const before = unixNow();
    expect((await createGroup(bearer, a, "YTE")).status).toBe(201);

    // B joins with a push: the group moves to rev 2 and B gets A's slot.
    const joined = await syncOk(bearer, b, {}, "YjE");
    expect(joined.rev).toBe(2);
    expect(joined.machines).toEqual([a, b].sort());
    expect(joined.slots).toHaveLength(1);
    expect(joined.slots[0]).toMatchObject({ machine: a, rev: 1, blob: "YTE" });
    const { written_at } = joined.slots[0];
    expect(written_at).toBeGreaterThanOrEqual(before);
    expect(written_at).toBeLessThanOrEqual(unixNow());

    // A pulls without pushing: B's slot comes back, its own never does, and rev stays put.
    const pulled = await syncOk(bearer, a, { [a]: 1 });
    expect(pulled.rev).toBe(2);
    expect(pulled.slots.map(({ machine, rev, blob }) => ({ machine, rev, blob }))).toEqual([
      { machine: b, rev: 2, blob: "YjE" },
    ]);
    expect(Object.keys(pulled.slots[0])).toEqual(["machine", "rev", "written_at", "blob"]);

    // Up to date: nothing to send.
    expect((await syncOk(bearer, a, { [a]: 1, [b]: 2 })).slots).toEqual([]);

    // B pushes again; only the newer slot goes to A.
    expect((await syncOk(bearer, b, { [a]: 1 }, "YjI")).rev).toBe(3);
    const second = await syncOk(bearer, a, { [a]: 1, [b]: 2 });
    expect(second.slots.map((s) => [s.machine, s.rev, s.blob])).toEqual([[b, 3, "YjI"]]);

    // A third machine reading with an empty have gets every other slot, sorted by machine,
    // and appears in `machines` only once it has written.
    const fresh = await syncOk(bearer, c);
    expect(fresh.machines).toEqual([a, b].sort());
    expect(fresh.slots.map((s) => s.machine)).toEqual([a, b].sort());
    const written = await syncOk(bearer, c, { [a]: 1, [b]: 3 }, "YzE");
    expect(written.rev).toBe(4);
    expect(written.machines).toEqual([a, b, c].sort());
    expect(written.slots).toEqual([]);
  });

  it("counts a missing have entry as 0 and ignores entries for unknown machines", async () => {
    const bearer = newBearer();
    const [a, b] = [newMachine(), newMachine()];
    await createGroup(bearer, a, "YTE");
    const body = await syncOk(bearer, b, { [newMachine()]: 99 });
    expect(body.slots.map((s) => [s.machine, s.rev])).toEqual([[a, 1]]);
  });

  it("takes a null blob as no blob", async () => {
    const bearer = newBearer();
    const [a, b] = [newMachine(), newMachine()];
    await createGroup(bearer, a);
    const res = await post("/v1/sync", bearer, { machine: b, have: {}, blob: null });
    expect(res.status).toBe(200);
    expect(((await res.json()) as SyncBody).rev).toBe(1);
  });

  it("refuses a sixth machine, even read-only, while the five keep working", async () => {
    const bearer = newBearer();
    const five = [newMachine(), newMachine(), newMachine(), newMachine(), newMachine()];
    await createGroup(bearer, five[0]);
    for (const m of five.slice(1)) await syncOk(bearer, m, {}, "AAAA");

    const sixth = newMachine();
    await expectError(await sync(bearer, sixth), 409, "full");
    await expectError(await sync(bearer, sixth, {}, "AAAA"), 409, "full");

    const body = await syncOk(bearer, five[0]);
    expect(body.rev).toBe(5);
    expect(body.machines).toEqual([...five].sort());
    expect((await syncOk(bearer, five[3], {}, "QkJCQg")).rev).toBe(6);
  });

  it("drops a slot not written for 90 days, which frees its place", async () => {
    const bearer = newBearer();
    const five = [newMachine(), newMachine(), newMachine(), newMachine(), newMachine()];
    await createGroup(bearer, five[0]);
    for (const m of five.slice(1)) await syncOk(bearer, m, {}, "AAAA");

    const [stale, recent] = [five[1], five[2]];
    await runInDurableObject(await groupOf(bearer), (_, state) => {
      const now = unixNow();
      state.storage.sql.exec("UPDATE slots SET written_at = ? WHERE machine = ?", now - 90 * DAY - 1, stale);
      state.storage.sql.exec("UPDATE slots SET written_at = ? WHERE machine = ?", now - 89 * DAY, recent);
    });

    const body = await syncOk(bearer, five[0]);
    expect(body.machines).toEqual(five.filter((m) => m !== stale).sort());
    expect(body.slots.map((s) => s.machine)).not.toContain(stale);
    expect(body.slots.map((s) => s.machine)).toContain(recent);

    // Four slots left, so a new machine fits again.
    const newcomer = newMachine();
    expect((await syncOk(bearer, newcomer, {}, "AAAA")).machines).toContain(newcomer);
  });
});

// ---------------------------------------------------------------------------------------------
// Expiry alarm
// ---------------------------------------------------------------------------------------------

describe("alarm", () => {
  const ninetyDaysOut = () => Date.now() + 90 * DAY * 1000;

  it("is set 90 days out on create, and pushed back by a write but not by a read", async () => {
    const bearer = newBearer();
    const [a, b] = [newMachine(), newMachine()];
    await createGroup(bearer, a);
    const created = (await storageOf(bearer)).alarm;
    expect(created).toBeGreaterThan(ninetyDaysOut() - 60_000);
    expect(created).toBeLessThanOrEqual(ninetyDaysOut());

    const soon = Date.now() + 60_000;
    await runInDurableObject(await groupOf(bearer), (_, state) => state.storage.setAlarm(soon));

    await syncOk(bearer, b);
    expect((await storageOf(bearer)).alarm).toBe(soon);

    await syncOk(bearer, b, {}, "AAAA");
    expect((await storageOf(bearer)).alarm).toBeGreaterThan(ninetyDaysOut() - 60_000);
  });

  it("deletes the whole group when it fires", async () => {
    const bearer = newBearer();
    const [a, b] = [newMachine(), newMachine()];
    await createGroup(bearer, a);
    await syncOk(bearer, b, {}, "AAAA");

    expect(await runDurableObjectAlarm(await groupOf(bearer))).toBe(true);

    await expectNoStorage(bearer);
    await expectError(await sync(bearer, a), 404, "no_group");
    // Nothing is left over: the same bearer could start a brand-new group.
    expect((await createGroup(bearer, b)).status).toBe(201);
    expect((await syncOk(bearer, a)).rev).toBe(1);
  });
});

// ---------------------------------------------------------------------------------------------
// GET /verify
// ---------------------------------------------------------------------------------------------

describe("GET /verify", () => {
  it("serves the Turnstile page with the configured site key", async () => {
    const res = await exports.default.fetch(`${ORIGIN}/verify`);
    expect(res.status).toBe(200);
    expect(res.headers.get("Content-Type")).toBe("text/html; charset=utf-8");
    expect(res.headers.get("Cache-Control")).toBe("no-store");
    const html = await res.text();
    expect(html).toContain('data-sitekey="1x00000000000000000000AA"');
    expect(html).toContain('src="https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit');
    expect(html).toContain('location.assign("/verify/done?token=" + encodeURIComponent(token))');
    expect(html).toContain("Turn on MyTube sync");
    expect(html).toContain("Confirm you're a person. MyTube then creates your sync group.");
    expect(siteverifyCalls).toHaveLength(0);
  });

  it("escapes the site key", async () => {
    const res = await fetchWithEnv(new Request(`${ORIGIN}/verify`), {
      TURNSTILE_SITE_KEY: `"><script>alert(1)</script>`,
    });
    const html = await res.text();
    expect(html).toContain('data-sitekey="&#34;&#62;&#60;script&#62;alert(1)&#60;/script&#62;"');
    expect(html).not.toContain("<script>alert(1)");
  });

  it("answers 503 with an explanation when no site key is configured", async () => {
    const res = await fetchWithEnv(new Request(`${ORIGIN}/verify`), { TURNSTILE_SITE_KEY: "" });
    expect(res.status).toBe(503);
    expect(res.headers.get("Content-Type")).toBe("text/html; charset=utf-8");
    expect(res.headers.get("Cache-Control")).toBe("no-store");
    const html = await res.text();
    expect(html).toContain("isn't configured");
    expect(html).not.toContain("api.js");
  });
});
