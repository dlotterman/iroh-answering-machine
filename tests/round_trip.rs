//! Real Iroh endpoints on loopback, no relays and no network: answering machines, their owners and senders, end to end,
//! through the same functions the `am` commands call.
// Tests may unwrap (see clippy.toml); clippy's exemption covers #[test] functions but not their helpers.
#![allow(clippy::unwrap_used)]
use std::{net::SocketAddr, sync::Arc};

use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, address_lookup::memory::MemoryLookup, protocol::Router};
use iroh_answering_machine::{
    invite, now_unix,
    owner::{self, LocalMessage},
    protocol::{
        EXIT_REFUSED, EXIT_UNREACHABLE, LEAVE_ALPN, LeaveRequest, MessageList, OWNER_ALPN, OwnerRequest, Receipt,
        Status, call, exit_code, message_text, text_message,
    },
    relays::bind_local,
    sender::{self, Leave, ReplyInvite},
    server::{self, MESSAGES_PER_MINUTE, Machine},
    state::{OwnerConfig, State, append_jsonl},
    store::Store,
};
use serde_json::json;
use tempfile::TempDir;

/// The test's network: every endpoint registers its loopback address here and finds the others through it, the way
/// production endpoints find each other through Datum's relays.
#[derive(Clone, Default)]
struct Net(MemoryLookup);

impl Net {
    async fn endpoint(&self, key: SecretKey, alpns: Vec<Vec<u8>>) -> Endpoint {
        let endpoint = bind_local(key, alpns, self.0.clone()).await.unwrap();
        let port = endpoint.bound_sockets().iter().find(|s| s.is_ipv4()).expect("an IPv4 socket").port();
        self.0
            .add_endpoint_info(EndpointAddr::new(endpoint.id()).with_ip_addr(SocketAddr::from(([127, 0, 0, 1], port))));
        endpoint
    }

    /// Someone with a state directory, dialing with its sender key: what `am leave` does.
    async fn person(&self) -> Person {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(dir.path().to_path_buf());
        let endpoint = self.endpoint(state.sender_key().unwrap(), vec![]).await;
        Person { state, endpoint, _dir: dir }
    }

    /// A running answering machine and its owner, set up as `am owner init` + `manifest` + `connect` would leave them.
    async fn owned_machine(&self, owner_name: &str) -> OwnedMachine {
        let person = self.person().await;
        let owner_id = owner::init(&person.state, owner_name).unwrap();
        let server = self.endpoint(SecretKey::generate(), vec![LEAVE_ALPN.to_vec(), OWNER_ALPN.to_vec()]).await;
        let server_id = server.id();
        let machine = Machine::new(server_id, vec![owner_id], vec![], Store::in_memory().unwrap());
        let router = server::spawn(server, Arc::new(machine));
        let config = OwnerConfig { name: owner_name.into(), server: Some(server_id.to_string()), ..Default::default() };
        person.state.save_owner_config(&config).unwrap();
        let owner_endpoint = self.endpoint(person.state.owner_key().unwrap(), vec![]).await;
        OwnedMachine { server_id, owner: person, owner_endpoint, _router: router }
    }
}

struct Person {
    state: State,
    /// Holds the sender key: for leaving messages and replies.
    endpoint: Endpoint,
    _dir: TempDir,
}

struct OwnedMachine {
    server_id: EndpointId,
    owner: Person,
    /// Holds the owner key: for the owner door.
    owner_endpoint: Endpoint,
    _router: Router,
}

impl OwnedMachine {
    fn invite(&self, label: &str) -> String {
        owner::make_invite(&self.owner.state, label, 24).unwrap().text
    }

    async fn fetch(&self) -> Vec<owner::LocalMessage> {
        owner::fetch(&self.owner.state, &self.owner_endpoint).await.unwrap()
    }

    async fn waiting(&self) -> i64 {
        owner::status(&self.owner.state, &self.owner_endpoint).await.unwrap().waiting
    }
}

async fn leave(from: &Person, invite: &str, text: &str) -> anyhow::Result<sender::Left> {
    let leave = Leave {
        to: invite.to_string(),
        text: text.to_string(),
        from: Some("Codex, for Bob".into()),
        reply_invite: ReplyInvite::None,
        context_id: None,
    };
    sender::leave(&from.state, &from.endpoint, leave).await
}

/// A leave request as a misbehaving client might send it, bypassing `sender::leave`'s own checks.
async fn raw_leave(from: &Person, server: EndpointId, request: LeaveRequest) -> anyhow::Result<Receipt> {
    call(&from.endpoint, server, LEAVE_ALPN, &request).await
}

fn request(invite: &str, text: &str) -> LeaveRequest {
    LeaveRequest { invite: invite.into(), from: None, reply_invite: None, message: text_message(text, None) }
}

#[tokio::test]
async fn a_message_is_left_fetched_once_and_kept_locally() {
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.person().await;
    let invite = ada.invite("Bob's agent");

    let left = leave(&bob, &invite, "Hello from Bob's agent").await.unwrap();
    assert_eq!(left.receipt.owner_name, "Ada");

    let new = ada.fetch().await;
    assert_eq!(new.len(), 1);
    let m = &new[0];
    assert_eq!(m.number, 1);
    assert_eq!(m.stored.sender_endpoint_id, bob.endpoint.id().to_string(), "the sender id comes from the handshake");
    assert_eq!(m.stored.invite_label, "Bob's agent");
    assert_eq!(m.stored.from.as_deref(), Some("Codex, for Bob"));

    assert!(ada.fetch().await.is_empty(), "fetched messages are gone from the server");
    assert_eq!(ada.waiting().await, 0);
    assert_eq!(owner::history(&ada.owner.state).unwrap().len(), 1, "the local copy keeps acked messages");

    leave(&bob, &invite, "second").await.unwrap();
    assert_eq!(ada.fetch().await[0].number, 2, "local numbers keep counting");
}

#[tokio::test]
async fn the_owner_door_closes_on_anyone_else() {
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let stranger = net.person().await;
    let err = call::<Status>(&stranger.endpoint, ada.server_id, OWNER_ALPN, &OwnerRequest::Status).await.unwrap_err();
    assert!(err.to_string().contains("not an owner"), "got: {err}");
    // The owner's own sender key is not the owner key either.
    let err = call::<Status>(&ada.owner.endpoint, ada.server_id, OWNER_ALPN, &OwnerRequest::Status).await.unwrap_err();
    assert!(err.to_string().contains("not an owner"), "got: {err}");
}

#[tokio::test]
async fn bad_invites_are_refused_and_nothing_is_stored() {
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.person().await;
    let ada_key = ada.owner.state.owner_key().unwrap();
    let now = now_unix();
    let signed_by = |key: &SecretKey, server: EndpointId, expires_at: u64| {
        invite::create(key, invite::new_body(server, "Ada", "someone", expires_at)).unwrap()
    };

    let cases = [
        (
            "signed by a stranger",
            signed_by(&SecretKey::generate(), ada.server_id, now + 3600),
            "not signed by an owner",
        ),
        ("expired", signed_by(&ada_key, ada.server_id, now.saturating_sub(1)), "expired"),
        (
            "for another machine",
            signed_by(&ada_key, SecretKey::generate().public(), now + 3600),
            "different answering machine",
        ),
        ("not an invite at all", "am1.garbage".to_string(), "damaged"),
    ];
    for (case, bad_invite, expected) in cases {
        let err = raw_leave(&bob, ada.server_id, request(&bad_invite, "hi")).await.unwrap_err().to_string();
        assert!(err.contains(expected), "{case}: expected {expected:?}, got {err:?}");
    }
    assert_eq!(ada.waiting().await, 0);
}

#[tokio::test]
async fn a_message_without_text_is_refused() {
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.person().await;
    let mut data_only = request(&ada.invite("Bob's agent"), "unused");
    data_only.message = json!({"parts": [{"kind": "data", "data": {"a": 1}}]});
    let err = raw_leave(&bob, ada.server_id, data_only).await.unwrap_err();
    assert!(err.to_string().contains("no text"), "got: {err}");
    assert_eq!(ada.waiting().await, 0);
}

#[tokio::test]
async fn a_reply_invite_must_be_a_real_invite() {
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.person().await;
    let mut bad_reply = request(&ada.invite("Bob's agent"), "hi");
    bad_reply.reply_invite = Some("am1.notreal".into());
    let err = raw_leave(&bob, ada.server_id, bad_reply).await.unwrap_err();
    assert!(err.to_string().contains("reply invite"), "got: {err}");
}

#[tokio::test]
async fn one_sender_is_limited_per_minute() {
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.person().await;
    // Separate invites, so only the per-sender limit can be what stops the last one.
    for i in 0..MESSAGES_PER_MINUTE {
        leave(&bob, &ada.invite(&format!("invite {i}")), "hi").await.unwrap();
    }
    let err = leave(&bob, &ada.invite("one more"), "hi").await.unwrap_err();
    assert!(err.to_string().contains("too many"), "got: {err}");
}

#[tokio::test]
async fn fresh_keys_sharing_one_invite_are_limited_too() {
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let shared = ada.invite("posted somewhere public");
    for _ in 0..MESSAGES_PER_MINUTE {
        leave(&net.person().await, &shared, "hi").await.unwrap();
    }
    let err = leave(&net.person().await, &shared, "hi").await.unwrap_err();
    assert!(err.to_string().contains("too many"), "a new key does not get around the per-invite limit: {err}");
}

#[tokio::test]
async fn a_reply_goes_back_through_the_attached_invite() {
    // Ada and Bob each own an answering machine. Bob's message carries an invite to his; Ada's agent replies with it.
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.owned_machine("Bob").await;
    let to_ada = Leave {
        to: ada.invite("Bob's agent"),
        text: "Lunch Friday?".into(),
        from: None,
        reply_invite: ReplyInvite::MakeOne,
        context_id: None,
    };
    sender::leave(&bob.owner.state, &bob.owner.endpoint, to_ada).await.unwrap();

    let at_ada = ada.fetch().await;
    assert!(owner::render(&at_ada).contains("can reply: yes"));
    owner::reply(&ada.owner.state, &ada.owner.endpoint, at_ada[0].number, "Yes, noon works".into()).await.unwrap();

    let at_bob = bob.fetch().await;
    assert_eq!(at_bob[0].stored.invite_label, "Ada's assistant (replying to you)", "Bob signed this label himself");
    assert_eq!(at_bob[0].stored.message["contextId"], at_ada[0].stored.message["messageId"], "the reply is threaded");
    assert!(owner::render(&at_bob).contains("| Yes, noon works"));
    assert!(owner::render(&at_bob).contains("can reply: yes"), "and Ada attached her own reply invite");
}

#[tokio::test]
async fn a_sender_reusing_a_message_id_loses_nothing() {
    // The A2A messageId is the sender's choice, so it cannot be what tells two messages apart.
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.person().await;
    let invite = ada.invite("Bob's agent");
    for text in ["first", "second"] {
        let mut same_id = request(&invite, text);
        same_id.message["messageId"] = json!("1");
        raw_leave(&bob, ada.server_id, same_id).await.unwrap();
    }
    let texts: Vec<String> = ada.fetch().await.iter().map(|m| message_text(&m.stored.message)).collect();
    assert_eq!(texts, ["first", "second"]);
}

#[tokio::test]
async fn a_lost_ack_neither_loses_nor_duplicates() {
    // Saved locally, but the ack never reached the server: the next fetch must skip it and still clear the server.
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.person().await;
    leave(&bob, &ada.invite("Bob's agent"), "only once").await.unwrap();
    let MessageList { messages } =
        call(&ada.owner_endpoint, ada.server_id, OWNER_ALPN, &OwnerRequest::List { since: 0 }).await.unwrap();
    let saved =
        LocalMessage { number: 1, fetched_at: 0, server: ada.server_id.to_string(), stored: messages[0].clone() };
    append_jsonl(&ada.owner.state.path("messages.jsonl"), &[saved]).unwrap();

    assert!(ada.fetch().await.is_empty(), "already saved, so nothing new");
    assert_eq!(owner::history(&ada.owner.state).unwrap().len(), 1, "and not saved twice");
    assert_eq!(ada.waiting().await, 0, "but the server copy is acked now");
}

#[tokio::test]
async fn a_long_owner_name_still_gets_reply_invites() {
    let net = Net::default();
    let long_name = "A".repeat(invite::MAX_LABEL_CHARS);
    let ada = net.owned_machine(&long_name).await;
    let bob = net.owned_machine("Bob").await;
    let to_ada = Leave {
        to: ada.invite("Bob's agent"),
        text: "hi".into(),
        from: None,
        reply_invite: ReplyInvite::MakeOne,
        context_id: None,
    };
    sender::leave(&bob.owner.state, &bob.owner.endpoint, to_ada).await.unwrap();
    assert!(owner::render(&ada.fetch().await).contains("can reply: yes"));
}

#[tokio::test]
async fn exit_codes_tell_an_unreachable_machine_from_a_refusal() {
    // The Rust CLI book's advice (https://rust-cli.github.io/book/in-depth/exit-code.html): a script should be able to
    // tell "down" from "no" by the exit code alone.
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.person().await;

    let bad_invite = raw_leave(&bob, ada.server_id, request("am1.garbage", "hi")).await.unwrap_err();
    assert_eq!(exit_code(&bad_invite), EXIT_REFUSED, "{bad_invite:#}");
    let not_owner = call::<Status>(&bob.endpoint, ada.server_id, OWNER_ALPN, &OwnerRequest::Status).await.unwrap_err();
    assert_eq!(exit_code(&not_owner), EXIT_REFUSED, "{not_owner:#}");

    let nobody = SecretKey::generate().public(); // no address anywhere: nothing to dial
    let unreachable = call::<Status>(&ada.owner_endpoint, nobody, OWNER_ALPN, &OwnerRequest::Status).await.unwrap_err();
    assert_eq!(exit_code(&unreachable), EXIT_UNREACHABLE, "{unreachable:#}");
}

#[tokio::test]
async fn a_reply_invite_needs_your_own_deployed_answering_machine() {
    // An owner identity without a deployed machine is not enough, and the error should say which flag to drop.
    let net = Net::default();
    let ada = net.owned_machine("Ada").await;
    let bob = net.person().await;
    owner::init(&bob.state, "Bob").unwrap();
    let to_ada = Leave {
        to: ada.invite("Bob's agent"),
        text: "hi".into(),
        from: None,
        reply_invite: ReplyInvite::MakeOne,
        context_id: None,
    };
    let err = sender::leave(&bob.state, &bob.endpoint, to_ada).await.unwrap_err().to_string();
    assert!(err.contains("`--reply-invite auto` needs your own deployed answering machine"), "got: {err}");
}
