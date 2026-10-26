//! `am`: the answering machine (`serve`), its owner's tools (`owner …`), and leaving messages (`leave`, `invites …`).
#![forbid(unsafe_code)]

use std::{
    io::{IsTerminal, Read},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use iroh::{Endpoint, SecretKey};
use iroh_answering_machine::{
    format_unix, now_unix, owner,
    protocol::{self, Status},
    relays, sender, server,
    state::State,
};

const SKILL: &str = include_str!("../skills/answering-machine/SKILL.md");

#[derive(Parser)]
#[command(
    name = "am",
    version,
    about = "An answering machine for your coding agent, over Iroh through Datum's relays."
)]
struct Cli {
    /// Where keys, saved invites and fetched messages live. In the container this is the /state volume.
    #[arg(long, env = "AM_STATE", global = true)]
    state: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the answering machine (on Datum Compute). Reads SERVER_SECRET_KEY_HEX, OWNER_IDS, REVOKED_INVITES, AM_DB,
    /// RELAYS.
    Serve,
    /// The owner's tools: identity, invites, inbox.
    #[command(subcommand)]
    Owner(OwnerCommand),
    /// Leave a message on someone's answering machine.
    Leave {
        /// An invite (am1.…) or the name it was saved under with `am invites save`.
        to: String,
        /// The message. `-` reads it from stdin.
        #[arg(long)]
        text: String,
        /// Who you are, e.g. "Codex, for Human_B". Shown to the owner as your claim, not verified.
        #[arg(long)]
        from: Option<String>,
        /// An invite to your own answering machine so they can reply: `auto` makes one (needs `am owner` set up here),
        /// or give an invite or a saved name.
        #[arg(long)]
        reply_invite: Option<String>,
    },
    /// Invites to other people's answering machines, kept in the state volume.
    #[command(subcommand)]
    Invites(InvitesCommand),
    /// Print the endpoint id this state volume leaves messages from.
    Whoami,
    /// Print the agent skill (SKILL.md) for leaving messages and checking the inbox.
    Skill {
        /// The image the skill's commands should run; pin a digest for repeatability.
        #[arg(long, default_value = owner::DEFAULT_IMAGE)]
        image: String,
    },
}

#[derive(Subcommand)]
enum OwnerCommand {
    /// Make the owner identity (once) and set the name senders see. Prints OWNER_ID for the server's OWNER_IDS.
    Init {
        #[arg(long)]
        name: String,
    },
    /// Print OWNER_ID.
    Id,
    /// Print the Datum manifest (Network, Secret, Workload) for `datumctl apply -f -`. Makes the server key on first use
    /// and remembers the server's endpoint id. The output holds the server's secret key: pipe it, do not print it.
    Manifest {
        /// Name for the Datum resources: NAME (network) and NAME-am (secret, workload).
        #[arg(long)]
        name: String,
        #[arg(long, default_value = owner::DEFAULT_IMAGE)]
        image: String,
    },
    /// Print the server's settings as an env file, to run `am serve` yourself instead of on Datum (`--env-file`). Makes the
    /// server key on first use, like `manifest`. The output holds the server's secret key: write it to a mode-600 file.
    ServerEnv,
    /// Let another owner identity (`am owner id` on it) read messages and sign invites. Takes effect when the manifest is
    /// re-applied and the workload restarted.
    AddOwner { owner_id: String },
    /// Undo `add-owner`, likewise from the next manifest on.
    RemoveOwner { owner_id: String },
    /// List the owners the manifest will name.
    Owners,
    /// Refuse an invite before it expires (its id from `am owner invites`). Takes effect when the manifest is re-applied
    /// and the workload restarted.
    Revoke { invite_id: String },
    /// Remember the answering machine's endpoint id, after checking that it accepts this owner.
    Connect { endpoint_id: String },
    /// Ask the answering machine how it is doing.
    Status,
    /// Make an invite for someone. Prints the invite and a message to send them.
    Invite {
        /// Who it is for, e.g. "Human_B's agent". Shown with every message left with this invite.
        #[arg(long)]
        label: String,
        #[arg(long, default_value_t = owner::DEFAULT_INVITE_HOURS)]
        hours: u64,
        /// Print only the invite.
        #[arg(long)]
        quiet: bool,
    },
    /// List the invites made here: ids (for `revoke`), labels, expiry.
    Invites,
    /// Fetch new messages, save them here, delete them from the server, and print them.
    Inbox,
    /// Print saved messages.
    History {
        /// Only the last N.
        #[arg(long)]
        last: Option<usize>,
    },
    /// Reply to message N through the reply invite its sender attached.
    Reply {
        number: usize,
        /// The reply. `-` reads it from stdin.
        #[arg(long)]
        text: String,
    },
}

#[derive(Subcommand)]
enum InvitesCommand {
    /// Save an invite under NAME. The invite is read from stdin, so it stays out of command lines and process lists.
    Save { name: String },
    /// List saved invites.
    List,
    /// Delete a saved invite.
    Forget { name: String },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        // Platform log viewers (`datumctl compute logs`) show escape codes literally.
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "iroh_answering_machine=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let attempt = attempt(&cli.command);
    if let Err(e) = run(cli).await.context(attempt) {
        eprintln!("error: {e:#}");
        std::process::exit(protocol::exit_code(&e));
    }
}

/// What the command was trying to do, as the start of its error message: "cannot <attempt>: <reason>". The reasons
/// underneath never say "cannot" themselves, so each message reads as one sentence (after Canonical's error discipline,
/// https://canonical.github.io/rust-best-practices/error-and-panic-discipline.html).
fn attempt(command: &Command) -> &'static str {
    match command {
        Command::Serve => "cannot run the answering machine",
        Command::Leave { .. } => "cannot leave the message",
        Command::Whoami => "cannot read the sender identity",
        Command::Skill { .. } => "cannot print the skill",
        Command::Invites(command) => match command {
            InvitesCommand::Save { .. } => "cannot save the invite",
            InvitesCommand::List => "cannot list the saved invites",
            InvitesCommand::Forget { .. } => "cannot forget the invite",
        },
        Command::Owner(command) => match command {
            OwnerCommand::Init { .. } => "cannot set up the owner identity",
            OwnerCommand::Id => "cannot read the owner identity",
            OwnerCommand::Manifest { .. } => "cannot make the manifest",
            OwnerCommand::ServerEnv => "cannot make the server settings",
            OwnerCommand::AddOwner { .. } => "cannot add the owner",
            OwnerCommand::RemoveOwner { .. } => "cannot remove the owner",
            OwnerCommand::Owners => "cannot list the owners",
            OwnerCommand::Revoke { .. } => "cannot revoke the invite",
            OwnerCommand::Connect { .. } => "cannot connect to the answering machine",
            OwnerCommand::Status => "cannot get the answering machine's status",
            OwnerCommand::Invite { .. } => "cannot make the invite",
            OwnerCommand::Invites => "cannot list the invites",
            OwnerCommand::Inbox => "cannot fetch messages",
            OwnerCommand::History { .. } => "cannot read the saved messages",
            OwnerCommand::Reply { .. } => "cannot send the reply",
        },
    }
}

async fn run(cli: Cli) -> Result<()> {
    let state = State::new(cli.state.unwrap_or_else(default_state_dir));
    match cli.command {
        Command::Serve => server::run(server::Config::from_env()?).await,
        Command::Owner(command) => run_owner(&state, command).await,
        Command::Leave { to, text, from, reply_invite } => {
            let leave = sender::Leave {
                to,
                text: text_arg(text)?,
                from,
                reply_invite: sender::ReplyInvite::from_arg(reply_invite),
                context_id: None,
            };
            let left =
                dial_as(state.sender_key()?, async |endpoint| sender::leave(&state, endpoint, leave).await).await?;
            println!(
                "Message #{} left on {}'s answering machine (invite \"{}\", valid until {}).",
                left.receipt.id,
                left.receipt.owner_name,
                left.invite.body.label,
                format_unix(left.invite.body.expires_at)
            );
            Ok(())
        }
        Command::Invites(command) => run_invites(&state, command),
        Command::Skill { image } => {
            print!("{}", SKILL.replace("__IMAGE__", &image).replace("__GUIDE_URL__", owner::GUIDE_URL));
            Ok(())
        }
        Command::Whoami => {
            println!("{}", state.sender_key()?.public());
            Ok(())
        }
    }
}

/// Run `work` with an endpoint on Datum's relays under `key`, and close the endpoint cleanly afterwards.
async fn dial_as<T>(key: SecretKey, work: impl AsyncFnOnce(&Endpoint) -> Result<T>) -> Result<T> {
    let endpoint = relays::bind(key, vec![]).await?;
    let result = work(&endpoint).await;
    endpoint.close().await;
    result
}

async fn run_owner(state: &State, command: OwnerCommand) -> Result<()> {
    match command {
        OwnerCommand::Init { name } => {
            println!("OWNER_ID={}", owner::init(state, &name)?);
        }
        OwnerCommand::Manifest { name, image } => {
            let manifest = owner::manifest(state, &name, &image)?;
            print!("{}", manifest.yaml);
            eprintln!("SERVER_ENDPOINT_ID={}", manifest.server);
        }
        OwnerCommand::ServerEnv => {
            let settings = owner::server_env(state)?;
            print!("{}", settings.yaml);
            eprintln!("SERVER_ENDPOINT_ID={}", settings.server);
        }
        OwnerCommand::AddOwner { owner_id } => {
            owner::add_owner(state, &owner_id)?;
            println!("Added. Re-apply the manifest and restart the workload for it to take effect.");
        }
        OwnerCommand::RemoveOwner { owner_id } => {
            owner::remove_owner(state, &owner_id)?;
            println!("Removed. Re-apply the manifest and restart the workload for it to take effect.");
        }
        OwnerCommand::Owners => {
            println!("{}  (this volume)", state.owner_key()?.public());
            for id in state.owner_config()?.also_owners {
                println!("{id}");
            }
        }
        OwnerCommand::Revoke { invite_id } => {
            owner::revoke(state, &invite_id)?;
            println!(
                "Revoked. Run `am owner inbox` first, then re-apply the manifest and restart the workload: \
                 the restart replaces the instance, and unread messages go with it."
            );
        }
        OwnerCommand::Id => println!("OWNER_ID={}", state.owner_key()?.public()),
        OwnerCommand::Connect { endpoint_id } => {
            let status =
                dial_as(state.owner_key()?, async |endpoint| owner::connect(state, endpoint, &endpoint_id).await)
                    .await?;
            println!("Connected: this owner is accepted.");
            print_status(&status);
        }
        OwnerCommand::Status => {
            print_status(&dial_as(state.owner_key()?, async |endpoint| owner::status(state, endpoint).await).await?)
        }
        OwnerCommand::Invite { label, hours, quiet } => {
            let owner::NewInvite { text: invite, body } = owner::make_invite(state, &label, hours)?;
            if quiet {
                println!("{invite}");
            } else {
                println!("Invite {} for \"{}\", valid until {}.", body.id, body.label, format_unix(body.expires_at));
                println!("Give your human this message to send:\n");
                println!("{}", owner::handoff_message(&body.owner_name, &invite, body.expires_at));
            }
        }
        OwnerCommand::Invites => {
            let issued = owner::issued_invites(state)?;
            if issued.is_empty() {
                println!("No invites made here yet.");
            }
            let revoked = state.owner_config()?.revoked_invites;
            for invite in issued {
                let status = if revoked.contains(&invite.id) {
                    "revoked"
                } else if invite.expires_at <= now_unix() {
                    "expired"
                } else {
                    "valid"
                };
                println!("{}  {status:7}  until {}  {}", invite.id, format_unix(invite.expires_at), invite.label);
            }
        }
        OwnerCommand::Inbox => {
            let new = dial_as(state.owner_key()?, async |endpoint| owner::fetch(state, endpoint).await).await?;
            print!("{}", owner::render(&new));
            if !new.is_empty() {
                println!("{} new message(s), saved in the state volume (messages.jsonl).", new.len());
            }
        }
        OwnerCommand::History { last } => {
            let all = owner::history(state)?;
            let skip = last.map_or(0, |n| all.len().saturating_sub(n));
            let shown = &all[skip..];
            if shown.is_empty() {
                println!("No saved messages.");
            } else {
                print!("{}", owner::render(shown));
            }
        }
        OwnerCommand::Reply { number, text } => {
            let text = text_arg(text)?;
            let left = dial_as(state.sender_key()?, async |endpoint| owner::reply(state, endpoint, number, text).await)
                .await?;
            println!("Reply #{} left on {}'s answering machine.", left.receipt.id, left.receipt.owner_name);
        }
    }
    Ok(())
}

fn run_invites(state: &State, command: InvitesCommand) -> Result<()> {
    match command {
        InvitesCommand::Save { name } => {
            let text = read_stdin("the invite")?;
            if text.trim().is_empty() {
                bail!(
                    "there was no invite on stdin; pipe it in, e.g. `printf %s \"$INVITE\" | am invites save {name}`"
                );
            }
            let invite = sender::save_invite(state, &name, &text)?;
            println!(
                "Saved \"{name}\": an invite to {}'s answering machine, labelled \"{}\", valid until {}.",
                invite.body.owner_name,
                invite.body.label,
                format_unix(invite.body.expires_at)
            );
        }
        InvitesCommand::List => {
            let saved = sender::saved_invites(state)?;
            if saved.is_empty() {
                println!("No saved invites.");
            }
            for sender::SavedInvite { name, invite } in saved {
                match invite {
                    Ok(i) if i.body.expires_at > now_unix() => {
                        println!(
                            "{name}: {}'s answering machine, until {}",
                            i.body.owner_name,
                            format_unix(i.body.expires_at)
                        )
                    }
                    Ok(i) => println!(
                        "{name}: {}'s answering machine, EXPIRED {}",
                        i.body.owner_name,
                        format_unix(i.body.expires_at)
                    ),
                    Err(e) => println!("{name}: unreadable ({e})"),
                }
            }
        }
        InvitesCommand::Forget { name } => {
            std::fs::remove_file(state.invite_path(&name)?)
                .with_context(|| format!("there is no saved invite named {name:?}"))?;
            println!("Forgot \"{name}\".");
        }
    }
    Ok(())
}

fn print_status(status: &Status) {
    println!("Answering machine {} ({} owner(s)).", status.endpoint_id, status.owners);
    println!("Waiting on the server: {} message(s), {} bytes.", status.waiting, status.bytes);
    if let Some(oldest) = status.oldest_received_at {
        println!("Oldest waiting since {}.", format_unix(oldest));
    }
}

/// `--text -` reads the text from stdin, which keeps long or awkwardly quoted messages out of the shell.
fn text_arg(text: String) -> Result<String> {
    let text = if text == "-" { read_stdin("the message")? } else { text };
    if text.trim().is_empty() {
        bail!("the message is empty");
    }
    Ok(text)
}

/// Read all of stdin. A person at a terminal gets told what is wanted and how to finish, rather than a silent wait
/// (after the Rust CLI book, "Communicating with machines": https://rust-cli.github.io/book/in-depth/machine-communication.html).
fn read_stdin(what: &str) -> Result<String> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprintln!("Reading {what} from the terminal; end with Ctrl-D.");
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text)?;
    Ok(text)
}

fn default_state_dir() -> PathBuf {
    let container = PathBuf::from("/state");
    if container.is_dir() {
        return container;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/share/answering-machine")
}
