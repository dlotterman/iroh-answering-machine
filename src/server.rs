//! `am serve`: the answering machine. Two doors on one Iroh endpoint, and a SQLite mailbox behind them.
//!
//! - The leave door ([`LEAVE_ALPN`]) takes anyone's connection, but each message must carry a valid owner-signed invite.
//! - The owner door ([`OWNER_ALPN`]) closes any connection whose handshake id is not in `OWNER_IDS`, before reading a byte.
//!
//! Logs carry ids, labels and sizes, never message text.
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use iroh::{
    Endpoint, EndpointId, SecretKey,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
};
use serde::{Serialize, de::DeserializeOwned};
use tracing::{info, warn};

use crate::{
    invite, keys,
    protocol::{
        Acked, CLOSE_NOT_OWNER, LEAVE_ALPN, LeaveRequest, MAX_FROM_CHARS, MAX_REQUEST_BYTES, MessageList,
        NOT_OWNER_REASON, OWNER_ALPN, OwnerRequest, Receipt, Reply, Status, normalize_message, read_json, write_json,
    },
    store::{Store, StoredMessage},
};

/// Messages per minute, per sender endpoint and per invite. An answering machine, not a chat line.
pub const MESSAGES_PER_MINUTE: usize = 10;
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Prune the rate limiter's table past this many keys, so a flood of fresh keys cannot grow it without bound.
const RATE_TABLE_PRUNE_AT: usize = 10_000;
const RELAY_WAIT: Duration = Duration::from_secs(30);
const DEFAULT_DB: &str = "/var/lib/answering-machine/inbox.db";

/// Everything `am serve` is told, all from the environment (the Datum Secret sets these).
pub struct Config {
    /// `SERVER_SECRET_KEY_HEX`: fixes the endpoint id, so invites survive restarts. Required.
    pub key: SecretKey,
    /// `OWNER_IDS`: who may read messages and whose signatures make an invite valid. At least one.
    pub owners: Vec<EndpointId>,
    /// `REVOKED_INVITES`: invite ids refused even before they expire.
    pub revoked_invites: Vec<String>,
    /// `AM_DB`: the SQLite file; lost with the instance.
    pub db: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("SERVER_SECRET_KEY_HEX")
            .map_err(|_| anyhow!("SERVER_SECRET_KEY_HEX is not set: without it the endpoint id, and every invite, would change on restart"))?;
        let key = keys::parse_secret_key(&key).context("SERVER_SECRET_KEY_HEX")?;
        let owners = list_env("OWNER_IDS").iter().map(|id| keys::parse_endpoint_id(id)).collect::<Result<Vec<_>>>()?;
        if owners.is_empty() {
            bail!(
                "OWNER_IDS is empty: nobody could read the messages or sign invites. Set it to the owner's endpoint id"
            );
        }
        let db = std::env::var("AM_DB").map(PathBuf::from).unwrap_or_else(|_| DEFAULT_DB.into());
        Ok(Self { key, owners, revoked_invites: list_env("REVOKED_INVITES"), db })
    }
}

/// A comma- or space-separated list; empty or unset is an empty list.
fn list_env(name: &str) -> Vec<String> {
    let value = std::env::var(name).unwrap_or_default();
    value.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// Everything both doors share.
#[derive(Debug)]
pub struct Machine {
    id: EndpointId,
    owners: Vec<EndpointId>,
    revoked_invites: Vec<String>,
    store: Store,
    limiter: RateLimiter,
}

impl Machine {
    pub fn new(id: EndpointId, owners: Vec<EndpointId>, revoked_invites: Vec<String>, store: Store) -> Self {
        Self { id, owners, revoked_invites, store, limiter: RateLimiter::new(MESSAGES_PER_MINUTE, RATE_WINDOW) }
    }

    fn leave(&self, sender: EndpointId, request: LeaveRequest) -> Result<Receipt> {
        let invite = invite::decode(&request.invite)?;
        invite.check(self.id, &self.owners, &self.revoked_invites, crate::now_unix())?;
        // Both limits are checked (and counted) so that neither a new key nor a shared invite gets around them.
        let sender_ok = self.limiter.allow(&format!("sender:{sender}"));
        let invite_ok = self.limiter.allow(&format!("invite:{}", invite.body.id));
        if !sender_ok || !invite_ok {
            bail!("too many messages; wait a minute and try again");
        }
        let message = normalize_message(&request.message)?;
        let from = request.from.map(|f| f.trim().to_string()).filter(|f| !f.is_empty());
        if from.as_ref().is_some_and(|f| f.chars().count() > MAX_FROM_CHARS) {
            bail!("`from` is limited to {MAX_FROM_CHARS} characters");
        }
        let reply_invite = checked_reply_invite(request.reply_invite)?;
        let stored = StoredMessage {
            id: 0,
            received_at: crate::now_unix(),
            sender_endpoint_id: sender.to_string(),
            invite_label: invite.body.label.clone(),
            invite_id: invite.body.id.clone(),
            from,
            reply_invite,
            message,
        };
        let id = self.store.insert(&stored)?;
        info!(id, sender = %sender.fmt_short(), invite = %invite.body.id, label = %invite.body.label, "message left");
        Ok(Receipt { id, owner_name: invite.body.owner_name })
    }

    fn owner(&self, request: OwnerRequest) -> Result<OwnerReply> {
        Ok(match request {
            OwnerRequest::List { since } => OwnerReply::Messages(MessageList { messages: self.store.list(since)? }),
            OwnerRequest::Ack { ids } => {
                let deleted = self.store.ack(&ids)?;
                info!(deleted, "messages acknowledged");
                OwnerReply::Acked(Acked { deleted })
            }
            OwnerRequest::Status => {
                let stats = self.store.stats()?;
                OwnerReply::Status(Status {
                    endpoint_id: self.id.to_string(),
                    waiting: stats.waiting,
                    bytes: stats.bytes,
                    oldest_received_at: stats.oldest_received_at,
                    owners: self.owners.len(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                })
            }
        })
    }
}

/// A reply invite must at least be a well-formed, correctly signed invite. Whose it is cannot be checked here.
fn checked_reply_invite(reply_invite: Option<String>) -> Result<Option<String>> {
    let Some(text) = reply_invite.map(|r| r.trim().to_string()).filter(|r| !r.is_empty()) else { return Ok(None) };
    invite::decode(&text).map_err(|e| anyhow!("the reply invite is not usable: {e}"))?;
    Ok(Some(text))
}

/// The owner door's results, one per [`OwnerRequest`]; serialized as the result itself.
#[derive(Serialize)]
#[serde(untagged)]
enum OwnerReply {
    Messages(MessageList),
    Acked(Acked),
    Status(Status),
}

#[derive(Debug, Clone)]
struct LeaveDoor(Arc<Machine>);

#[derive(Debug, Clone)]
struct OwnerDoor(Arc<Machine>);

impl ProtocolHandler for LeaveDoor {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let sender = connection.remote_id();
        let machine = self.0.clone();
        answer_each_stream(connection, move |request| {
            let receipt = machine.leave(sender, request);
            if let Err(e) = &receipt {
                info!(sender = %sender.fmt_short(), "message refused: {e:#}");
            }
            receipt
        })
        .await;
        Ok(())
    }
}

impl ProtocolHandler for OwnerDoor {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer = connection.remote_id();
        if !self.0.owners.contains(&peer) {
            warn!(peer = %peer.fmt_short(), "refused on the owner door: not an owner");
            connection.close(CLOSE_NOT_OWNER.into(), NOT_OWNER_REASON);
            return Ok(());
        }
        let machine = self.0.clone();
        answer_each_stream(connection, move |request| machine.owner(request)).await;
        Ok(())
    }
}

/// One JSON request and one JSON reply per stream, until the peer closes the connection. Each stream gets its own task.
async fn answer_each_stream<R, T>(connection: Connection, answer: impl Fn(R) -> Result<T> + Clone + Send + 'static)
where
    R: DeserializeOwned + Send + 'static,
    T: Serialize + Send + Sync + 'static,
{
    while let Ok((mut send, mut receive)) = connection.accept_bi().await {
        let answer = answer.clone();
        tokio::spawn(async move {
            let result = read_json::<R>(&mut receive, MAX_REQUEST_BYTES).await.and_then(answer);
            let _ = write_json(&mut send, &Reply::from(result)).await;
        });
    }
}

/// Serve both doors on `endpoint` until the returned router is shut down.
pub fn spawn(endpoint: Endpoint, machine: Arc<Machine>) -> Router {
    Router::builder(endpoint)
        .accept(LEAVE_ALPN, LeaveDoor(machine.clone()))
        .accept(OWNER_ALPN, OwnerDoor(machine))
        .spawn()
}

/// `am serve`: bind on Datum's relays and run until SIGTERM or Ctrl-C.
pub async fn run(config: Config) -> Result<()> {
    let store = Store::open(&config.db)?;
    let endpoint = crate::relays::bind(config.key, vec![LEAVE_ALPN.to_vec(), OWNER_ALPN.to_vec()]).await?;
    let id = endpoint.id();
    // The line the guide reads from `datumctl compute logs`; same format as the other Datum demos.
    println!("ENDPOINT_ID={id}");
    info!(owners = config.owners.len(), revoked = config.revoked_invites.len(), db = %config.db.display(), "answering machine up");
    let machine = Arc::new(Machine::new(id, config.owners, config.revoked_invites, store));
    let router = spawn(endpoint.clone(), machine);

    // Nobody can reach us until we are homed on a relay; on a fresh Datum network that waits on DNS, so say so.
    let watch = endpoint.clone();
    tokio::spawn(async move {
        match tokio::time::timeout(RELAY_WAIT, watch.online()).await {
            Ok(()) => info!("online: reachable through {:?}", watch.addr().relay_urls().collect::<Vec<_>>()),
            Err(_) => warn!("not connected to a relay after {}s: check DNS and outbound HTTPS", RELAY_WAIT.as_secs()),
        }
    });

    shutdown_signal().await;
    info!("shutting down");
    router.shutdown().await?;
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

/// A sliding-window counter per key.
#[derive(Debug)]
struct RateLimiter {
    max: usize,
    window: Duration,
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    fn new(max: usize, window: Duration) -> Self {
        Self { max, window, hits: Mutex::new(HashMap::new()) }
    }

    fn allow(&self, key: &str) -> bool {
        let now = Instant::now();
        // Like the store: a panic in another request must not leave the limiter unusable.
        let mut hits = self.hits.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if hits.len() > RATE_TABLE_PRUNE_AT {
            hits.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < self.window));
        }
        let times = hits.entry(key.to_string()).or_default();
        while times.front().is_some_and(|t| now.duration_since(*t) >= self.window) {
            times.pop_front();
        }
        if times.len() >= self.max {
            return false;
        }
        times.push_back(now);
        true
    }
}
