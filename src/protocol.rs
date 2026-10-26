//! The wire: two ALPNs, and one JSON request and one JSON reply per Iroh bi-directional stream.
//!
//! The client writes its request and finishes its side of the stream; the server reads to the end, writes its reply and
//! finishes. QUIC's end-of-stream is the framing, so there is no HTTP and no length prefix. A reply is
//! `{"ok": <result>}` or `{"error": "<message meant to be shown as is>"}` ([`Reply`]); each request has its own result
//! type, defined here so the server and the client agree on it.
//!
//! Messages are A2A `Message` objects (<https://a2a-protocol.org>): `kind`, `role`, `messageId`, optional `contextId`,
//! and `parts` of kind `text` or `data`. We use A2A's data model, not its transport.
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use iroh::{
    Endpoint, EndpointAddr, EndpointId,
    endpoint::{Connection, RecvStream, SendStream},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::store::StoredMessage;

/// Anyone holding a valid invite may leave a message here.
pub const LEAVE_ALPN: &[u8] = b"datum/answering-machine/leave/0";
/// Only the owners may connect here; anyone else is closed right after the handshake.
pub const OWNER_ALPN: &[u8] = b"datum/answering-machine/owner/0";

/// QUIC close code for a peer on the owner door that is not an owner.
pub const CLOSE_NOT_OWNER: u32 = 1;
pub const NOT_OWNER_REASON: &[u8] = b"not an owner of this answering machine";

pub const MAX_REQUEST_BYTES: usize = 32 * 1024;
pub const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 8 * 1024;
pub const MAX_PARTS: usize = 8;
pub const MAX_FROM_CHARS: usize = 200;
pub const MAX_ID_CHARS: usize = 100;

/// Exit codes from BSD sysexits, as the Rust CLI book suggests (https://rust-cli.github.io/book/in-depth/exit-code.html),
/// so a script can tell an answering machine that is down from one that said no without reading the message. Bad
/// command-line arguments exit 2 (clap's code).
pub const EXIT_FAILED: i32 = 1;
/// EX_UNAVAILABLE: the answering machine could not be reached.
pub const EXIT_UNREACHABLE: i32 = 69;
/// EX_NOPERM: the answering machine was reached and refused (a bad invite, not an owner, full, too many messages).
pub const EXIT_REFUSED: i32 = 77;

/// Why a call to an answering machine failed, carried inside the error so `main` can pick the exit code.
#[derive(Debug)]
pub enum CallFailure {
    Unreachable(String),
    Refused(String),
}

impl std::fmt::Display for CallFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallFailure::Unreachable(why) | CallFailure::Refused(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for CallFailure {}

/// The exit code for an error: [`EXIT_UNREACHABLE`] or [`EXIT_REFUSED`] if a call failed that way, else [`EXIT_FAILED`].
pub fn exit_code(error: &anyhow::Error) -> i32 {
    match error.chain().find_map(|cause| cause.downcast_ref::<CallFailure>()) {
        Some(CallFailure::Unreachable(_)) => EXIT_UNREACHABLE,
        Some(CallFailure::Refused(_)) => EXIT_REFUSED,
        None => EXIT_FAILED,
    }
}

/// Relays can take a few seconds to introduce two endpoints; much longer means the other side is not there.
const DIAL_TIMEOUT: Duration = Duration::from_secs(30);
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Leave door request: one message, with the invite that lets it in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaveRequest {
    pub invite: String,
    /// Who the sender says it is. Not verified: shown to the owner as the sender's own claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// An invite to the sender's own answering machine, so the owner can reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_invite: Option<String>,
    pub message: Value,
}

/// Leave door result: the message's id on the server, and whose answering machine took it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub id: i64,
    pub owner_name: String,
}

/// Owner door requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OwnerRequest {
    /// Messages still on the server with an id above `since`, oldest first, a page at a time. Result: [`MessageList`].
    List { since: i64 },
    /// Delete these messages from the server; the owner has saved them. Result: [`Acked`].
    Ack { ids: Vec<i64> },
    /// Result: [`Status`].
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageList {
    pub messages: Vec<StoredMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Acked {
    pub deleted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub endpoint_id: String,
    pub waiting: i64,
    pub bytes: i64,
    pub oldest_received_at: Option<u64>,
    pub owners: usize,
    pub version: String,
}

/// Every reply on the wire.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply<T> {
    Ok(T),
    Error(String),
}

impl<T> From<Result<T>> for Reply<T> {
    fn from(result: Result<T>) -> Self {
        match result {
            Ok(value) => Reply::Ok(value),
            Err(e) => Reply::Error(format!("{e:#}")),
        }
    }
}

pub async fn read_json<T: DeserializeOwned>(receive: &mut RecvStream, limit: usize) -> Result<T> {
    let bytes = receive.read_to_end(limit).await.map_err(|e| anyhow!("the request broke off: {e}"))?;
    serde_json::from_slice(&bytes).context("the request is not the JSON this door expects")
}

pub async fn write_json(send: &mut SendStream, value: &impl Serialize) -> Result<()> {
    send.write_all(&serde_json::to_vec(value)?).await?;
    send.finish()?;
    Ok(())
}

/// Dial `server` on `alpn`, send one request and return its result. Failures carry a [`CallFailure`]: unreachable, or
/// refused (an error reply, or the owner door closing on us).
///
/// The endpoint finds `server` through its address lookup: Datum's relays in production, loopback in tests.
pub async fn call<R: DeserializeOwned>(
    endpoint: &Endpoint,
    server: EndpointId,
    alpn: &[u8],
    request: &impl Serialize,
) -> Result<R> {
    let name = server.fmt_short();
    let unreachable = |why: String| anyhow::Error::new(CallFailure::Unreachable(why));
    let connection = tokio::time::timeout(DIAL_TIMEOUT, endpoint.connect(EndpointAddr::new(server), alpn))
        .await
        .map_err(|_| {
            unreachable(format!(
                "answering machine {name} did not answer within {}s; is it running?",
                DIAL_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| unreachable(format!("the connection to answering machine {name} failed: {e}")))?;
    let exchange = async {
        let (mut send, mut receive) = connection.open_bi().await?;
        write_json(&mut send, request).await?;
        let bytes = receive.read_to_end(MAX_REPLY_BYTES).await?;
        anyhow::Ok(serde_json::from_slice::<Reply<R>>(&bytes)?)
    };
    let reply = match tokio::time::timeout(REPLY_TIMEOUT, exchange).await {
        Err(_) => {
            return Err(unreachable(format!(
                "answering machine {name} did not reply within {}s",
                REPLY_TIMEOUT.as_secs()
            )));
        }
        Ok(Err(e)) => {
            return Err(why_closed(&connection).unwrap_or_else(|| unreachable(format!("the exchange broke off: {e}"))));
        }
        Ok(Ok(reply)) => reply,
    };
    connection.close(0u32.into(), b"done");
    match reply {
        Reply::Ok(result) => Ok(result),
        Reply::Error(message) => Err(CallFailure::Refused(format!("the answering machine refused: {message}")).into()),
    }
}

/// A refusal right after the handshake (the owner door's) shows up as a closed connection; say why, not just "closed".
fn why_closed(connection: &Connection) -> Option<anyhow::Error> {
    let reason = connection.close_reason()?;
    Some(CallFailure::Refused(format!("the answering machine closed the connection: {reason}")).into())
}

/// An A2A text message.
pub fn text_message(text: &str, context_id: Option<&str>) -> Value {
    let mut message = json!({
        "kind": "message",
        "role": "user",
        "messageId": crate::random_hex(16),
        "parts": [{"kind": "text", "text": text}],
    });
    if let Some(context_id) = context_id {
        message["contextId"] = json!(context_id);
    }
    message
}

/// Check a sender's A2A message and rebuild it with only the fields we keep, so nothing unexpected is stored.
pub fn normalize_message(message: &Value) -> Result<Value> {
    let parts = message["parts"].as_array().ok_or_else(|| anyhow!("the message's parts are not a list"))?;
    if parts.is_empty() || parts.len() > MAX_PARTS {
        bail!("the message must have 1 to {MAX_PARTS} parts");
    }
    let mut text_bytes = 0;
    let mut kept = Vec::with_capacity(parts.len());
    for part in parts {
        match part["kind"].as_str() {
            Some("text") => {
                let text = part["text"].as_str().ok_or_else(|| anyhow!("a text part has no text field"))?;
                text_bytes += text.len();
                kept.push(json!({"kind": "text", "text": text}));
            }
            Some("data") if part["data"].is_object() => kept.push(json!({"kind": "data", "data": part["data"]})),
            _ => bail!("a part must be {{\"kind\":\"text\",\"text\":…}} or {{\"kind\":\"data\",\"data\":{{…}}}}"),
        }
    }
    if text_bytes == 0 {
        bail!("the message has no text");
    }
    if text_bytes > MAX_TEXT_BYTES {
        bail!("the message text is {text_bytes} bytes; the limit is {MAX_TEXT_BYTES}");
    }
    let message_id = short_id(&message["messageId"]).unwrap_or_else(|| crate::random_hex(16));
    let mut normalized = json!({"kind": "message", "role": "user", "messageId": message_id, "parts": kept});
    if let Some(context_id) = short_id(&message["contextId"]) {
        normalized["contextId"] = json!(context_id);
    }
    Ok(normalized)
}

/// The text parts of a message, joined.
pub fn message_text(message: &Value) -> String {
    let texts: Vec<&str> =
        message["parts"].as_array().into_iter().flatten().filter_map(|part| part["text"].as_str()).collect();
    texts.join("\n")
}

fn short_id(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty() && s.chars().count() <= MAX_ID_CHARS).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizing_keeps_text_and_data_and_drops_everything_else() {
        let incoming = json!({
            "kind": "message", "role": "agent", "messageId": "m1", "contextId": "c1", "extensions": ["x"],
            "parts": [{"kind": "text", "text": "hi", "metadata": {"evil": true}}, {"kind": "data", "data": {"a": 1}}],
        });
        let message = normalize_message(&incoming).unwrap();
        assert_eq!(
            message,
            json!({"kind": "message", "role": "user", "messageId": "m1", "contextId": "c1",
                   "parts": [{"kind": "text", "text": "hi"}, {"kind": "data", "data": {"a": 1}}]})
        );
    }

    #[test]
    fn bad_messages_are_refused() {
        let too_long = "x".repeat(MAX_TEXT_BYTES + 1);
        for bad in [
            json!({}),
            json!({"parts": []}),
            json!({"parts": [{"kind": "data", "data": {"only": "data"}}]}),
            json!({"parts": [{"kind": "file", "file": {}}]}),
            json!({"parts": [{"kind": "text", "text": too_long}]}),
        ] {
            assert!(normalize_message(&bad).is_err(), "{bad} should be refused");
        }
    }

    use proptest::prelude::*;

    /// Any JSON at all.
    fn any_json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::from),
            any::<i64>().prop_map(Value::from),
            ".{0,20}".prop_map(Value::from),
        ];
        leaf.prop_recursive(4, 64, 8, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
                prop::collection::btree_map(".{0,8}", inner, 0..8).prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        })
    }

    /// JSON shaped like a message, so the checks past "is it an object with parts" get exercised too.
    fn message_shaped() -> impl Strategy<Value = Value> {
        let part = (
            prop_oneof![Just("text".to_string()), Just("data".to_string()), Just("file".to_string()), ".{0,6}"],
            any_json(),
            any_json(),
        )
            .prop_map(|(kind, text, data)| json!({"kind": kind, "text": text, "data": data}));
        (prop::collection::vec(part, 0..12), any_json(), any_json())
            .prop_map(|(parts, id, context)| json!({"parts": parts, "messageId": id, "contextId": context}))
    }

    // Messages are written by strangers: refuse what is wrong, never panic, and store only what normalizing promises
    // (ANSSI LANG-LIMIT-PANIC; proptest, from the Rust Secure Code WG's project list).
    proptest! {
        #[test]
        fn normalizing_never_panics_and_keeps_only_text_and_data(message in prop_oneof![any_json(), message_shaped()]) {
            if let Ok(normalized) = normalize_message(&message) {
                let parts = normalized["parts"].as_array().cloned().unwrap_or_default();
                prop_assert!((1..=MAX_PARTS).contains(&parts.len()));
                for part in parts {
                    let kind = part["kind"].as_str().unwrap_or_default();
                    prop_assert!(kind == "text" || kind == "data", "kept a {kind:?} part");
                }
                prop_assert!(!message_text(&normalized).is_empty());
            }
        }
    }
}
