//! Leaving messages on someone's answering machine, and the invites saved for doing so.
use anyhow::{Result, bail};
use iroh::Endpoint;

use crate::{
    invite::{self, Invite},
    protocol::{LEAVE_ALPN, LeaveRequest, Receipt, call, text_message},
    state::State,
};

/// What to attach so the recipient's agent can answer.
pub enum ReplyInvite {
    None,
    /// Make a fresh invite to our own answering machine (`--reply-invite auto`).
    MakeOne,
    /// An invite, or the name one was saved under.
    Given(String),
}

impl ReplyInvite {
    /// The `--reply-invite` argument: absent, `auto`, or an invite / saved name.
    pub fn from_arg(arg: Option<String>) -> Self {
        match arg.as_deref() {
            None => ReplyInvite::None,
            Some("auto") => ReplyInvite::MakeOne,
            Some(_) => ReplyInvite::Given(arg.unwrap_or_default()),
        }
    }
}

pub struct Leave {
    /// An invite, or the name it was saved under.
    pub to: String,
    pub text: String,
    pub from: Option<String>,
    pub reply_invite: ReplyInvite,
    /// A2A contextId, to thread a conversation. Replies set it; a first message has none.
    pub context_id: Option<String>,
}

/// What the answering machine said when it took the message, and the invite that got it in.
#[derive(Debug)]
pub struct Left {
    pub receipt: Receipt,
    pub invite: Invite,
}

/// Decode an invite and refuse it here, with a clear reason, if it has already expired.
pub fn usable_invite(text: &str) -> Result<Invite> {
    let invite = invite::decode(text)?;
    if invite.body.expires_at <= crate::now_unix() {
        bail!(
            "the invite to {}'s answering machine expired at {}; ask them for a new one",
            invite.body.owner_name,
            crate::format_unix(invite.body.expires_at)
        );
    }
    Ok(invite)
}

/// `am leave`: leave one message, dialing from `endpoint` (which should hold this volume's sender key).
pub async fn leave(state: &State, endpoint: &Endpoint, leave: Leave) -> Result<Left> {
    let invite_text = state.resolve_invite(&leave.to)?;
    let invite = usable_invite(&invite_text)?;
    let reply_invite = match leave.reply_invite {
        ReplyInvite::None => None,
        ReplyInvite::MakeOne => Some(crate::owner::reply_invite_for(state, &invite.body.owner_name)?),
        ReplyInvite::Given(name_or_invite) => Some(state.resolve_invite(&name_or_invite)?),
    };
    let request = LeaveRequest {
        invite: invite_text,
        from: leave.from,
        reply_invite,
        message: text_message(&leave.text, leave.context_id.as_deref()),
    };
    let receipt = call(endpoint, invite.server()?, LEAVE_ALPN, &request).await?;
    Ok(Left { receipt, invite })
}

/// `am invites save NAME`: check the invite, then keep it in the state volume (mode 600).
pub fn save_invite(state: &State, name: &str, text: &str) -> Result<Invite> {
    let invite = usable_invite(text)?;
    let canonical: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    crate::keys::write_private(&state.invite_path(name)?, canonical.as_bytes())?;
    Ok(invite)
}

/// An invite saved with `am invites save`, decoded, or the reason it could not be.
#[derive(Debug)]
pub struct SavedInvite {
    pub name: String,
    pub invite: Result<Invite>,
}

/// `am invites list`: saved invites, by name.
pub fn saved_invites(state: &State) -> Result<Vec<SavedInvite>> {
    let dir = state.invites_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let invite = std::fs::read_to_string(entry.path()).map_err(Into::into).and_then(|t| invite::decode(&t));
        out.push(SavedInvite { name, invite });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
