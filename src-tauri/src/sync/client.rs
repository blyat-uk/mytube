//! The two requests sync makes, and what every answer to them means.
//!
//! Both are `POST` with a JSON body and the code's bearer. Nothing here reads
//! or writes local state: the callers in `sync/mod.rs` decide what an answer
//! does to `sync.json`, which is what lets these be tested against a fake
//! server with nothing else set up.

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, RETRY_AFTER, USER_AGENT};
use serde::{Deserialize, Serialize};

/// Where sync goes when `settings.json` names no `sync_server`.
pub const DEFAULT_SERVER: &str = "https://mytube-sync.blyat.uk";

/// The server keeps at most this many `have` entries, and refuses more.
pub const MAX_HAVE: usize = 5;

pub const NETWORK: &str = "Can't reach the sync server. MyTube will keep trying.";
pub const RATE_LIMITED: &str = "The sync server asked MyTube to slow down. Retrying in a minute.";
pub const SERVER: &str = "The sync server had a problem. MyTube will keep trying.";
pub const FULL: &str = "This sync group already has 5 computers.";
pub const NO_GROUP: &str = "No sync group uses this code. Check it and try again.";
pub const VERIFICATION_FAILED: &str = "Verification failed. Try again.";

/// The server address with no trailing slash. Empty means the public server;
/// anything else is a hand edit (a `wrangler dev` on localhost, a self-hosted
/// Worker) and is taken as written.
pub fn base_url(setting: &str) -> String {
    let s = setting.trim().trim_end_matches('/');
    if s.is_empty() { DEFAULT_SERVER.to_string() } else { s.to_string() }
}

/// Every way a request can fail, one variant per row of the spec's table.
/// `Display` is the sentence the status line shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// No answer at all: offline, DNS, refused, timed out. The detail is for
    /// the journal only.
    Network(String),
    /// The edge's rate limit. Its answer is an HTML page, so the header is all
    /// there is to read.
    RateLimited { retry_after: Option<Duration> },
    /// A 5xx, or an answer this build cannot make sense of.
    Server(u16),
    /// `404 no_group`: no group uses this code, or it expired.
    NoGroup,
    /// `409 exists` on create: astronomically unlikely with a fresh 128-bit code.
    Exists,
    /// `409 full`: this machine has no slot, and five others do.
    Full,
    /// `403 verification_failed`: Turnstile rejected the token.
    VerificationFailed,
    /// A 4xx the protocol has no meaning for here (`bad_request`, `too_large`):
    /// this build and the server disagree about the request.
    Refused { status: u16, error: String },
}

impl ClientError {
    /// How long the server asked for, when it said.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(_) => f.write_str(NETWORK),
            Self::RateLimited { .. } => f.write_str(RATE_LIMITED),
            Self::Server(_) => f.write_str(SERVER),
            Self::NoGroup => f.write_str(NO_GROUP),
            Self::Exists => f.write_str("Could not create a sync group. Try again."),
            Self::Full => f.write_str(FULL),
            Self::VerificationFailed => f.write_str(VERIFICATION_FAILED),
            Self::Refused { status, error } => {
                write!(f, "The sync server refused the request ({status} {error}). Updating MyTube may fix this.")
            }
        }
    }
}

impl std::error::Error for ClientError {}

/// One other machine's slot, newer than this machine's `have` said.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Slot {
    pub machine: String,
    pub rev: u64,
    pub written_at: i64,
    pub blob: String,
}

/// A `200` from `/v1/sync`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SyncResponse {
    pub rev: u64,
    /// Every slot present, the caller's included.
    pub machines: Vec<String>,
    pub slots: Vec<Slot>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupRequest<'a> {
    turnstile_token: &'a str,
    machine: &'a str,
    blob: &'a str,
}

#[derive(Serialize)]
struct SyncRequest<'a> {
    machine: &'a str,
    have: BTreeMap<&'a str, u64>,
    /// Absent, not null, when there is nothing new to push: a pull that sends
    /// no blob does not bump the group's rev or the slot's expiry.
    #[serde(skip_serializing_if = "Option::is_none")]
    blob: Option<&'a str>,
}

#[derive(Deserialize)]
struct GroupResponse {
    #[allow(dead_code)]
    rev: u64,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

/// `POST /v1/group`: creates the group with this machine's slot as rev 1.
pub async fn create_group(
    http: &reqwest::Client,
    base: &str,
    bearer: &str,
    turnstile_token: &str,
    machine: &str,
    blob: &str,
) -> Result<(), ClientError> {
    let body = GroupRequest { turnstile_token, machine, blob };
    let (status, text) = post(http, &format!("{base}/v1/group"), bearer, &body).await?;
    serde_json::from_str::<GroupResponse>(&text).map_err(|_| ClientError::Server(status))?;
    Ok(())
}

/// `POST /v1/sync`: pushes `blob` if given, and pulls every other slot newer
/// than `have`.
pub async fn sync(
    http: &reqwest::Client,
    base: &str,
    bearer: &str,
    machine: &str,
    have: &BTreeMap<String, u64>,
    blob: Option<&str>,
) -> Result<SyncResponse, ClientError> {
    let body = SyncRequest { machine, have: have_to_send(have, machine), blob };
    let (status, text) = post(http, &format!("{base}/v1/sync"), bearer, &body).await?;
    serde_json::from_str(&text).map_err(|_| ClientError::Server(status))
}

/// At most [`MAX_HAVE`] entries, never this machine's own. `have` is pruned to
/// the group's machines after every answer, so this only bites on a file
/// edited by hand -- and dropping an entry costs nothing but a slot read again.
fn have_to_send<'a>(have: &'a BTreeMap<String, u64>, me: &str) -> BTreeMap<&'a str, u64> {
    have.iter()
        .filter(|(m, _)| m.as_str() != me)
        .take(MAX_HAVE)
        .map(|(m, r)| (m.as_str(), *r))
        .collect()
}

/// Sends one request and sorts the answer into success (the status and body
/// of a 2xx) or a [`ClientError`].
async fn post(
    http: &reqwest::Client,
    url: &str,
    bearer: &str,
    body: &impl Serialize,
) -> Result<(u16, String), ClientError> {
    let json = serde_json::to_vec(body).map_err(|e| ClientError::Network(e.to_string()))?;
    let resp = http
        .post(url)
        .bearer_auth(bearer)
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, concat!("mytube/", env!("CARGO_PKG_VERSION")))
        .body(json)
        .send()
        .await
        .map_err(|e| ClientError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let text = resp.text().await.map_err(|e| ClientError::Network(e.to_string()))?;
    match classify(status, retry_after.as_deref(), &text) {
        None => Ok((status, text)),
        Some(err) => Err(err),
    }
}

/// `None` for a success; otherwise what the answer means.
///
/// Every error is matched on the status *and* the body's `error`: a 404 is
/// only "no such group" when the Worker says so, since a misrouted request or
/// a wrong `sync_server` gets a 404 too and must not mark a live group expired.
fn classify(status: u16, retry_after: Option<&str>, body: &str) -> Option<ClientError> {
    if (200..300).contains(&status) {
        return None;
    }
    let error = serde_json::from_str::<ErrorBody>(body).map(|b| b.error).unwrap_or_default();
    Some(match (status, error.as_str()) {
        (429, _) => ClientError::RateLimited { retry_after: retry_after.and_then(parse_retry_after) },
        (404, "no_group") => ClientError::NoGroup,
        (409, "exists") => ClientError::Exists,
        (409, "full") => ClientError::Full,
        (403, "verification_failed") => ClientError::VerificationFailed,
        (400 | 413, e) if !e.is_empty() => ClientError::Refused { status, error: error.clone() },
        _ => ClientError::Server(status),
    })
}

/// `Retry-After` in its seconds form. The HTTP-date form is not something the
/// edge sends, and reads as "not said".
fn parse_retry_after(v: &str) -> Option<Duration> {
    v.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A request as the fake server saw it.
    #[derive(Debug)]
    pub(crate) struct Seen {
        pub method: String,
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: serde_json::Value,
    }

    impl Seen {
        pub(crate) fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        }
    }

    /// One canned answer.
    pub(crate) struct Reply {
        pub status: u16,
        pub headers: Vec<(&'static str, String)>,
        pub body: String,
    }

    pub(crate) fn reply(status: u16, body: &str) -> Reply {
        Reply { status, headers: vec![], body: body.to_string() }
    }

    /// A one-thread HTTP/1.1 server that answers each connection with the next
    /// canned reply, in order, and reports every request it read. Plain std:
    /// it only has to speak enough HTTP for reqwest, and a real server crate
    /// would be the larger part of what this tests.
    pub(crate) fn fake_server(replies: Vec<Reply>) -> (String, mpsc::Receiver<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for r in replies {
                let Ok((stream, _)) = listener.accept() else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        headers.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
                let len: usize = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let body = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                let _ = tx.send(Seen { method, path, headers, body });

                let mut out = stream;
                let mut head = format!(
                    "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                    r.status,
                    r.body.len()
                );
                for (k, v) in &r.headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                let _ = out.write_all(head.as_bytes());
                let _ = out.write_all(r.body.as_bytes());
            }
        });
        (base, rx)
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap()
    }

    const BEARER: &str = "Llmm4-FtqobVZwturlPDbjcnyu1fxNo2db_fNmeYVAs";
    const ME: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn have(n: usize) -> BTreeMap<String, u64> {
        (0..n).map(|i| (format!("machine{i:015}"), i as u64 + 1)).collect()
    }

    async fn sync_against(r: Reply, blob: Option<&str>) -> (Result<SyncResponse, ClientError>, Seen) {
        let (base, seen) = fake_server(vec![r]);
        let out = sync(&http(), &base, BEARER, ME, &have(2), blob).await;
        (out, seen.recv().unwrap())
    }

    async fn group_against(r: Reply) -> (Result<(), ClientError>, Seen) {
        let (base, seen) = fake_server(vec![r]);
        let out = create_group(&http(), &base, BEARER, "XXXX.DUMMY.TOKEN.XXXX", ME, "AQID").await;
        (out, seen.recv().unwrap())
    }

    #[test]
    fn base_url_defaults_and_loses_a_trailing_slash() {
        assert_eq!(base_url(""), DEFAULT_SERVER);
        assert_eq!(base_url("  "), DEFAULT_SERVER);
        assert_eq!(base_url("http://127.0.0.1:8787/"), "http://127.0.0.1:8787");
        assert_eq!(base_url(" https://sync.example.test "), "https://sync.example.test");
    }

    #[tokio::test]
    async fn a_group_request_carries_the_bearer_the_token_the_machine_and_the_blob() {
        let (out, seen) = group_against(reply(201, r#"{"rev":1}"#)).await;
        out.unwrap();
        assert_eq!((seen.method.as_str(), seen.path.as_str()), ("POST", "/v1/group"));
        assert_eq!(seen.header("authorization"), Some(format!("Bearer {BEARER}").as_str()));
        assert_eq!(seen.header("content-type"), Some("application/json"));
        assert_eq!(
            seen.body,
            serde_json::json!({"turnstileToken": "XXXX.DUMMY.TOKEN.XXXX", "machine": ME, "blob": "AQID"})
        );
    }

    #[tokio::test]
    async fn group_errors_map_to_their_rows() {
        let cases = [
            (reply(403, r#"{"error":"verification_failed"}"#), ClientError::VerificationFailed),
            (reply(409, r#"{"error":"exists"}"#), ClientError::Exists),
            (reply(400, r#"{"error":"bad_request"}"#),
             ClientError::Refused { status: 400, error: "bad_request".into() }),
            (reply(413, r#"{"error":"too_large"}"#),
             ClientError::Refused { status: 413, error: "too_large".into() }),
            (reply(502, "<html>bad gateway</html>"), ClientError::Server(502)),
            // A 201 whose body is not the protocol's is the server's problem.
            (reply(201, "<html>"), ClientError::Server(201)),
        ];
        for (r, want) in cases {
            assert_eq!(group_against(r).await.0.unwrap_err(), want);
        }
    }

    #[tokio::test]
    async fn a_sync_with_a_blob_sends_it_and_reads_the_slots_back() {
        let body = r#"{"rev":9,"machines":["AAAAAAAAAAAAAAAAAAAAAA","BBBBBBBBBBBBBBBBBBBBBB"],
                       "slots":[{"machine":"BBBBBBBBBBBBBBBBBBBBBB","rev":8,"written_at":1700000000,"blob":"AQID"}]}"#;
        let (out, seen) = sync_against(reply(200, body), Some("BLOB")).await;
        let resp = out.unwrap();
        assert_eq!(resp.rev, 9);
        assert_eq!(resp.machines.len(), 2);
        assert_eq!(
            resp.slots,
            vec![Slot { machine: "B".repeat(22), rev: 8, written_at: 1_700_000_000, blob: "AQID".into() }]
        );
        assert_eq!(seen.path, "/v1/sync");
        assert_eq!(seen.header("authorization"), Some(format!("Bearer {BEARER}").as_str()));
        assert_eq!(seen.body["machine"], ME);
        assert_eq!(seen.body["blob"], "BLOB");
        assert_eq!(seen.body["have"], serde_json::json!({"machine000000000000000": 1, "machine000000000000001": 2}));
    }

    #[tokio::test]
    async fn a_pull_leaves_the_blob_out_entirely() {
        let (out, seen) = sync_against(reply(200, r#"{"rev":1,"machines":[],"slots":[]}"#), None).await;
        out.unwrap();
        let fields = seen.body.as_object().unwrap();
        assert!(!fields.contains_key("blob"), "{fields:?}");
    }

    #[tokio::test]
    async fn an_empty_have_is_still_sent_as_an_empty_object() {
        // The server requires the field: a missing `have` is a 400.
        let (base, seen) = fake_server(vec![reply(200, r#"{"rev":1,"machines":[],"slots":[]}"#)]);
        sync(&http(), &base, BEARER, ME, &BTreeMap::new(), Some("B")).await.unwrap();
        assert_eq!(seen.recv().unwrap().body["have"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn have_is_capped_at_five_and_never_names_this_machine() {
        let (base, seen) = fake_server(vec![reply(200, r#"{"rev":1,"machines":[],"slots":[]}"#)]);
        let mut h = have(8);
        h.insert(ME.into(), 99);
        sync(&http(), &base, BEARER, ME, &h, None).await.unwrap();
        let sent = seen.recv().unwrap().body["have"].as_object().unwrap().clone();
        assert_eq!(sent.len(), MAX_HAVE);
        assert!(!sent.contains_key(ME));
    }

    #[tokio::test]
    async fn sync_errors_map_to_their_rows() {
        let cases = [
            (reply(404, r#"{"error":"no_group"}"#), ClientError::NoGroup),
            (reply(409, r#"{"error":"full"}"#), ClientError::Full),
            // Any other 404 is a wrong address or route, not an expired group.
            (reply(404, r#"{"error":"not_found"}"#), ClientError::Server(404)),
            (reply(404, "Not Found"), ClientError::Server(404)),
            (reply(500, r#"{"error":"internal"}"#), ClientError::Server(500)),
            (reply(503, ""), ClientError::Server(503)),
            (reply(200, r#"{"rev":"nine"}"#), ClientError::Server(200)),
            // The edge's block page for a custom rule.
            (reply(403, "<html>blocked</html>"), ClientError::Server(403)),
        ];
        for (r, want) in cases {
            assert_eq!(sync_against(r, None).await.0.unwrap_err(), want);
        }
    }

    #[tokio::test]
    async fn a_429_reads_retry_after_in_seconds() {
        let mut r = reply(429, "<html>rate limited</html>");
        r.headers.push(("Retry-After", "17".into()));
        let err = sync_against(r, None).await.0.unwrap_err();
        assert_eq!(err, ClientError::RateLimited { retry_after: Some(Duration::from_secs(17)) });
        assert_eq!(err.retry_after(), Some(Duration::from_secs(17)));
        assert_eq!(err.to_string(), RATE_LIMITED);

        let bare = sync_against(reply(429, ""), None).await.0.unwrap_err();
        assert_eq!(bare, ClientError::RateLimited { retry_after: None });

        let mut dated = reply(429, "");
        dated.headers.push(("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT".into()));
        assert_eq!(sync_against(dated, None).await.0.unwrap_err().retry_after(), None);
    }

    #[tokio::test]
    async fn nothing_listening_is_a_network_error() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let err = sync(&http(), &format!("http://127.0.0.1:{port}"), BEARER, ME, &have(0), None)
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::Network(_)), "{err:?}");
        assert_eq!(err.to_string(), NETWORK);
    }

    #[tokio::test]
    async fn a_server_that_never_answers_is_a_network_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let quick = reqwest::Client::builder().timeout(Duration::from_millis(300)).build().unwrap();
        let err = sync(&quick, &base, BEARER, ME, &have(0), None).await.unwrap_err();
        assert!(matches!(err, ClientError::Network(_)), "{err:?}");
        drop(listener);
    }

    #[test]
    fn the_user_facing_sentences_are_the_specs() {
        assert_eq!(ClientError::Network("x".into()).to_string(), NETWORK);
        assert_eq!(ClientError::Server(500).to_string(), SERVER);
        assert_eq!(ClientError::Full.to_string(), FULL);
        assert_eq!(ClientError::NoGroup.to_string(), NO_GROUP);
        assert_eq!(ClientError::VerificationFailed.to_string(), VERIFICATION_FAILED);
    }
}
