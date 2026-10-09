# mytube-sync

The server half of MyTube's opt-in cloud sync: a Cloudflare Worker at `mytube-sync.blyat.uk`
with one SQLite-backed Durable Object per sync group. Each group holds up to five slots, one per
computer, and each slot is a blob the app encrypted before upload — the server never sees a key
and can't read anything it stores. There are no accounts: a group is addressed by
`hex(SHA-256(bearer))`, where the bearer is derived from the sync code on the client. It runs on
the Workers Free plan, whose daily quota is shared with the owner's other projects, so the Worker
refuses anything malformed before it reaches a Durable Object, only `POST /v1/group` (behind
Turnstile) ever creates storage, logs nothing, and lets a group expire after 90 days without a
write.

## API

All JSON responses carry `Content-Type: application/json` and `Cache-Control: no-store`. Both
POST endpoints need `Authorization: Bearer <43 base64url chars>` and a JSON body of at most
720,000 bytes.

| Request | Body | Answers |
|---|---|---|
| `POST /v1/group` | `{"turnstileToken", "machine", "blob"}` | `201 {"rev":1}`, `403 verification_failed`, `409 exists` |
| `POST /v1/sync` | `{"machine", "have": {machine: rev}, "blob"?}` | `200 {"rev", "machines", "slots"}`, `404 no_group`, `409 full` |
| `GET /verify` | — | The Turnstile page; `503` while no site key is configured |
| anything else | — | `404 not_found` |

`machine` is 22 base64url chars, `blob` base64url without padding up to 699,051 chars, `have` at
most five entries of non-negative integers. A bad bearer or body is `400 bad_request`, an
oversized body `413 too_large`. `slots` holds the other machines' slots with
`rev > have[machine]` (missing counts as 0), each `{"machine", "rev", "written_at", "blob"}` with
`written_at` in unix seconds; `machines` lists every slot, the caller's included. A deployment
without `TURNSTILE_SECRET` answers `POST /v1/group` with `503 not_configured`.

## Local development

```sh
bun install
cp .dev.vars.example .dev.vars   # Cloudflare's always-pass Turnstile test keys
bun run dev                      # http://localhost:8787
```

With the test keys, `http://localhost:8787/verify` shows a widget that always passes, and
`POST /v1/group` accepts the dummy token `XXXX.DUMMY.TOKEN.XXXX` (the Worker still asks
Cloudflare's real siteverify endpoint, which honours the test secret). Durable Object data lives
in `.wrangler/state` and survives restarts; delete that directory for a clean slate. Point the
app at it with `"sync_server": "http://localhost:8787"` in its `settings.json`.

## Tests

```sh
bun run typecheck
bun run test
```

The suite runs in workerd through `@cloudflare/vitest-plugin`, against the real Worker and a real
SQLite Durable Object; only siteverify is mocked. It covers every validation rejection (and that
none of them touches storage or siteverify), group creation and `exists`, Turnstile pass and fail,
`no_group` leaving no storage, the push/pull round trip with `have` filtering, the five-machine
limit, 90-day slot expiry, the alarm deleting the group, unknown routes, and the `/verify` page.

## Deploying

Everything below is done once. The zone `blyat.uk` must already be on the account. Steps 1–4 are
wrangler; 5 and 6 are the dashboard, because wrangler's login can't read or change a zone's WAF
rules.

1. **Log in.** `bunx wrangler login`.
2. **Create the Turnstile widget**, somewhere outside this repository, since the output holds the
   secret key:

   ```sh
   bunx wrangler turnstile widget create "MyTube sync" --domain mytube-sync.blyat.uk \
     --mode managed --clearance-level no_clearance --json > ~/widget.json
   ```

   (Dashboard → *Turnstile* → *Add widget* does the same.)
3. **Put the site key** (`sitekey` in that output) in `wrangler.jsonc` as
   `vars.TURNSTILE_SITE_KEY` and commit it; it is public, and `/verify` answers 503 without it.
   The secret key never goes in a file in this repository (`.dev.vars` is git-ignored and holds
   only the test keys).
4. **Deploy with the secret.**

   ```sh
   jq '{TURNSTILE_SECRET: .secret}' ~/widget.json > ~/secrets.json
   bunx wrangler deploy --secrets-file ~/secrets.json
   rm ~/widget.json ~/secrets.json   # the dashboard shows the secret again if it is ever needed
   ```

   Wrangler attaches the custom domain `mytube-sync.blyat.uk`,
   creating its DNS record and certificate. `workers_dev` and `preview_urls` are off on purpose:
   a `*.workers.dev` or preview address would reach the same Worker without passing through the
   zone's WAF and rate-limit rules below, so anyone could spend the account's shared quota
   unfiltered.
5. **Add the custom rule.** Zone `blyat.uk` → *Security* → *WAF* → *Custom rules* → *Create rule*
   (in the newer dashboard: *Security* → *Security rules* → *Create rule* → *Custom rule*). Name
   it `mytube-sync: API only`, choose *Edit expression*, paste the expression below, set the
   action to **Block**, and deploy.

   ```
   (http.host eq "mytube-sync.blyat.uk" and not ((http.request.method eq "POST" and http.request.uri.path in {"/v1/group" "/v1/sync"}) or (http.request.method eq "GET" and http.request.uri.path eq "/verify")))
   ```

6. **Add the rate limiting rule.** Same place → *Rate limiting rules* → *Create rule*. Name it
   `mytube-sync: pace`, and set:
   - *If incoming requests match*: `http.request.uri.path in {"/v1/group" "/v1/sync"}`
     (*Edit expression*, or the URI Path field with the *is in* operator);
   - *With the same characteristics*: **IP**;
   - *When rate exceeds*: **1** request per **10 seconds**;
   - *Then take action*: **Block**, for **10 seconds**.

   The free plan fixes both the period and the block duration at 10 s, allows only the URI path
   field in the expression (so the rule can't name the host, and applies to those two paths on
   every host in the zone — nothing else in `blyat.uk` uses them), and allows a single rate
   limiting rule per zone. The app paces itself to one request a minute; this rule only stops a
   runaway or hostile client. A blocked request gets the edge's `429` page.
7. **Check it.**

   ```sh
   curl -i https://mytube-sync.blyat.uk/verify          # 200, the Turnstile page
   curl -i https://mytube-sync.blyat.uk/                # 403 from the custom rule
   curl -i -X POST https://mytube-sync.blyat.uk/v1/sync # 400 {"error":"bad_request"}
   curl -i -X POST https://mytube-sync.blyat.uk/v1/sync # again within 10 s: 429 from the edge
   ```

Requests the custom rule or the rate limiter block are answered at Cloudflare's edge and never
invoke the Worker, so they cost no Worker or Durable Object requests from the shared quota. Only
well-formed requests on the three allowed routes, at most one per IP every 10 seconds, ever
reach it.

Later deploys are just `bun run deploy`; the secret and the domain carry over. Observability is
off, so nothing is logged or retained; `bunx wrangler tail` shows live invocations when
debugging, and the Worker itself never prints a bearer or a blob.
