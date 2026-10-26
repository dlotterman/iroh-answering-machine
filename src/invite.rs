//! Invites: the one string a sender needs, signed by an owner so the server can check it with no state of its own.
//!
//! `am1.` + base32 of a postcard-encoded [`SignedInvite`]. The body names the server to dial, who the owner is, who the
//! invite was given to, and when it expires. The server accepts it if an id in its `OWNER_IDS` signed it, it names this
//! server, it has not expired and its id is not in `REVOKED_INVITES`. Nothing is stored, so invites survive restarts.
//!
//! An invite is a bearer secret: whoever holds it can leave messages until it expires.
use anyhow::{Result, anyhow, bail};
use iroh::{EndpointId, SecretKey, Signature};
use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "am1.";
const VERSION: u8 = 1;
/// Prepended to the body before signing, so an invite signature can never be mistaken for any other use of the key.
const SIGNING_CONTEXT: &[u8] = b"datum-answering-machine/invite/v1";
/// Longer than any real invite; a guard against pasting the wrong thing into the decoder.
const MAX_ENCODED_LEN: usize = 2048;
pub const MAX_LABEL_CHARS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteBody {
    pub version: u8,
    /// The answering machine's endpoint id: where to dial.
    pub server: [u8; 32],
    /// Whose answering machine this is, e.g. "Human_A".
    pub owner_name: String,
    /// Who the invite was given to, e.g. "Human_B's agent". Shown to the owner with every message left with it.
    pub label: String,
    pub expires_at: u64,
    /// Identifies this invite, for revocation.
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignedInvite {
    body: Vec<u8>,
    owner: [u8; 32],
    signature: Vec<u8>,
}

/// A decoded invite whose signature matches the key it names. Whether that key is one of the server's owners, and whether
/// the invite has expired, is for [`Invite::check`] to say.
#[derive(Debug, Clone)]
pub struct Invite {
    pub body: InviteBody,
    pub owner: EndpointId,
}

impl Invite {
    pub fn server(&self) -> Result<EndpointId> {
        EndpointId::from_bytes(&self.body.server).map_err(|e| anyhow!("the invite names an invalid server id: {e}"))
    }

    /// Why this server should refuse the invite, if it should.
    pub fn check(&self, server: EndpointId, owners: &[EndpointId], revoked: &[String], now: u64) -> Result<()> {
        if self.body.server != *server.as_bytes() {
            bail!("the invite is for a different answering machine");
        }
        if !owners.contains(&self.owner) {
            bail!("the invite was not signed by an owner of this answering machine");
        }
        if self.body.expires_at <= now {
            bail!("the invite expired at {}; ask its owner for a new one", crate::format_unix(self.body.expires_at));
        }
        if revoked.contains(&self.body.id) {
            bail!("the invite was revoked");
        }
        Ok(())
    }
}

pub fn create(owner_key: &SecretKey, body: InviteBody) -> Result<String> {
    if body.label.trim().is_empty() || body.label.chars().count() > MAX_LABEL_CHARS {
        bail!("the label must be 1 to {MAX_LABEL_CHARS} characters");
    }
    let body = postcard::to_stdvec(&body)?;
    let signature = owner_key.sign(&signed_bytes(&body)).to_bytes().to_vec();
    let signed = SignedInvite { body, owner: *owner_key.public().as_bytes(), signature };
    let encoded = data_encoding::BASE32_NOPAD.encode(&postcard::to_stdvec(&signed)?).to_ascii_lowercase();
    Ok(format!("{PREFIX}{encoded}"))
}

/// Decode an invite and check its signature. Whitespace from copy and paste is ignored.
pub fn decode(text: &str) -> Result<Invite> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(encoded) = text.strip_prefix(PREFIX) else {
        bail!("the invite does not start with {PREFIX}; copy all of it")
    };
    if encoded.len() > MAX_ENCODED_LEN {
        bail!("the text is too long to be an invite");
    }
    let bytes = data_encoding::BASE32_NOPAD
        .decode(encoded.to_ascii_uppercase().as_bytes())
        .map_err(|_| damaged("not base32"))?;
    let signed: SignedInvite = postcard::from_bytes(&bytes).map_err(|_| damaged("unreadable"))?;
    let owner = EndpointId::from_bytes(&signed.owner).map_err(|_| damaged("owner key"))?;
    let signature: [u8; 64] = signed.signature.as_slice().try_into().map_err(|_| damaged("signature"))?;
    owner
        .verify(&signed_bytes(&signed.body), &Signature::from_bytes(&signature))
        .map_err(|_| anyhow!("the invite's signature does not match: it was changed or damaged"))?;
    let body: InviteBody = postcard::from_bytes(&signed.body).map_err(|_| damaged("body"))?;
    if body.version != VERSION {
        bail!("this invite is version {}; this program reads version {VERSION}", body.version);
    }
    Ok(Invite { body, owner })
}

fn damaged(part: &str) -> anyhow::Error {
    anyhow!("the invite is damaged ({part}); copy it again in full")
}

pub fn new_body(server: EndpointId, owner_name: &str, label: &str, expires_at: u64) -> InviteBody {
    InviteBody {
        version: VERSION,
        server: *server.as_bytes(),
        owner_name: owner_name.to_string(),
        label: label.to_string(),
        expires_at,
        id: crate::random_hex(8),
    }
}

fn signed_bytes(body: &[u8]) -> Vec<u8> {
    [SIGNING_CONTEXT, body].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn setup() -> (SecretKey, EndpointId, String) {
        let owner = SecretKey::generate();
        let server = SecretKey::generate().public();
        let invite = create(&owner, new_body(server, "Ada", "Bob's agent", NOW + 3600)).unwrap();
        (owner, server, invite)
    }

    #[test]
    fn a_fresh_invite_round_trips_and_is_accepted() {
        let (owner, server, text) = setup();
        let invite = decode(&format!("  {text}\n")).unwrap();
        assert_eq!(invite.body.label, "Bob's agent");
        assert_eq!(invite.server().unwrap(), server);
        invite.check(server, &[owner.public()], &[], NOW).unwrap();
    }

    #[test]
    fn the_server_refuses_invites_it_should_not_honour() {
        let (owner, server, text) = setup();
        let invite = decode(&text).unwrap();
        let owners = [owner.public()];
        let revoked = std::slice::from_ref(&invite.body.id);
        let stranger = [SecretKey::generate().public()];
        let other_server = SecretKey::generate().public();
        let cases: [(&str, Result<()>, &str); 4] = [
            ("expired", invite.check(server, &owners, &[], NOW + 3600), "expired"),
            ("revoked", invite.check(server, &owners, revoked, NOW), "revoked"),
            ("another machine's", invite.check(other_server, &owners, &[], NOW), "different answering machine"),
            ("signed by a non-owner", invite.check(server, &stranger, &[], NOW), "not signed by an owner"),
        ];
        for (case, result, expected) in cases {
            let err = result.expect_err(case).to_string();
            assert!(err.contains(expected), "{case}: expected {expected:?}, got {err:?}");
        }
    }

    #[test]
    fn changing_a_signed_invite_breaks_its_signature() {
        let (_, _, text) = setup();
        let bytes = data_encoding::BASE32_NOPAD.decode(text[PREFIX.len()..].to_ascii_uppercase().as_bytes()).unwrap();
        let mut signed: SignedInvite = postcard::from_bytes(&bytes).unwrap();
        let mut body: InviteBody = postcard::from_bytes(&signed.body).unwrap();
        body.label = "Your boss".into();
        signed.body = postcard::to_stdvec(&body).unwrap();
        let encoded = data_encoding::BASE32_NOPAD.encode(&postcard::to_stdvec(&signed).unwrap()).to_ascii_lowercase();
        let err = decode(&format!("{PREFIX}{encoded}")).unwrap_err();
        assert!(err.to_string().contains("signature does not match"), "got: {err}");
    }

    // Invites arrive from other people, so decoding must refuse garbage, never panic on it (ANSSI LANG-LIMIT-PANIC).
    proptest::proptest! {
        #[test]
        fn decoding_any_text_never_panics(text in ".{0,400}") {
            let _ = decode(&text);
        }

        #[test]
        fn decoding_invite_shaped_text_never_panics(body in "[a-z2-7]{0,600}") {
            let _ = decode(&format!("{PREFIX}{body}"));
        }
    }
}
