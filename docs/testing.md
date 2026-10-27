# Testing

## Automated: `podman build --target check .`

fmt, clippy (warnings are errors, and `unwrap` outside tests is an error, per the `[lints]` table in `Cargo.toml`),
and the tests. Both crate roots `#![forbid(unsafe_code)]`. The property tests use
[proptest](https://github.com/proptest-rs/proptest), from the Rust Secure Code WG's list of projects.

The tests check intentions, not coverage. Each one names what it protects, and the security ones were checked by
breaking the behaviour and watching the test fail.

`tests/round_trip.rs` runs answering machines, owners and senders as real Iroh endpoints on loopback. They find each other
through iroh's in-memory address lookup instead of the relays, and the tests call the same functions as the `am` commands.

| Intent | Test |
| --- | --- |
| A message arrives, is fetched once, and stays in the owner's local copy | `a_message_is_left_fetched_once_and_kept_locally` |
| Only owners get in the owner door (not even the owner's own sender key) | `the_owner_door_closes_on_anyone_else` |
| Forged, expired, misdirected and garbled invites store nothing | `bad_invites_are_refused_and_nothing_is_stored` |
| The server's invite rules, and a changed invite fails its signature | `invite::tests::*` |
| Messages without text, and fake reply invites, are refused | `a_message_without_text_is_refused`, `a_reply_invite_must_be_a_real_invite` |
| Rate limits hold per sender, and per invite against fresh keys | `one_sender_is_limited_per_minute`, `fresh_keys_sharing_one_invite_are_limited_too` |
| A reply goes back through the attached invite, threaded | `a_reply_goes_back_through_the_attached_invite` |
| Long names still get reply invites | `a_long_owner_name_still_gets_reply_invites` |
| `--reply-invite auto` without a deployed machine says which flag to drop | `a_reply_invite_needs_your_own_deployed_answering_machine` |
| No message is lost or duplicated, whatever ids senders pick | `a_sender_reusing_a_message_id_loses_nothing`, `a_lost_ack_neither_loses_nor_duplicates` |
| Sender text cannot pass for a verified line: one hand-written attack, plus a property test over arbitrary and deliberately field-shaped text (each of the renderer's two defences removed in turn was caught) | `owner::tests::sender_text_cannot_pass_for_a_verified_line`, `owner::tests::no_sender_text_passes_for_a_verified_line` |
| Untrusted input never panics: any text as an invite, any JSON as a message; and normalizing keeps only what it promises (ANSSI LANG-LIMIT-PANIC; proptest) | `invite::tests::decoding_*_never_panics`, `protocol::tests::normalizing_never_panics_and_keeps_only_text_and_data` |
| A panic during one request leaves the store usable (the server now unwinds instead of aborting) | `store::tests::a_panic_while_holding_the_store_does_not_break_it` |
| A full mailbox refuses new messages and keeps the old | `store::tests::a_full_mailbox_refuses_new_messages_and_keeps_the_old_ones` |
| The manifest is complete, keeps its server id, revocations and owners, and refuses unsafe input or the wrong volume | `owner::tests::*` |
| Only text and data parts are stored | `protocol::tests::*` |
| Exit codes tell an unreachable machine (69) from a refusal (77) (Rust CLI book, exit codes) | `exit_codes_tell_an_unreachable_machine_from_a_refusal` |

`tests/cli.rs` runs the `am` binary itself, as the Rust CLI book's testing chapter recommends
(https://rust-cli.github.io/book/tutorial/testing.html). It checks the strings agents and the guides rely on, and the exit
codes. Like the book suggests, it checks for key strings rather than whole layouts, and never `--help`.

| Intent | Test |
| --- | --- |
| `OWNER_ID=` is printed and stays the same across `init` runs | `owner_init_prints_an_owner_id_that_stays_the_same` |
| Owner commands before `init` say how to start | `owner_commands_before_init_say_how_to_start` |
| The manifest goes to stdout and `SERVER_ENDPOINT_ID=` to stderr | `the_manifest_goes_to_stdout_and_the_server_id_to_stderr` |
| An invite piped into `invites save` is saved, with no terminal prompt when piped | `an_invite_travels_from_owner_to_sender_through_stdin` |
| An expired invite is refused locally, before anything is sent | `an_expired_invite_is_refused_before_anything_is_sent` |
| Revocations show in `owner invites`; malformed ids are refused | `a_revoked_invite_shows_as_revoked` |
| `am skill` prints a complete skill | `the_printed_skill_is_complete` |
| An empty history says so | `an_empty_history_says_so` |
| `serve` refuses to start without its key | `serve_refuses_to_start_without_its_key` |
| `owner server-env` carries exactly what the manifest would (self-hosting) | `server_env_matches_the_manifest_for_self_hosting` |
| Invite lifetimes are capped at 720 hours instead of overflowing (ANSSI LANG-ARITH) | `invite_lifetimes_are_capped_instead_of_overflowing` |
| Every error reads `cannot <attempt>: <why>`, with one "cannot", and local failures exit 1 (Canonical, error discipline) | `every_error_reads_as_one_sentence_and_local_failures_exit_1` |

Not covered by tests: the relays and Datum itself. The runs below cover those.

## Supply chain: `podman build --target audit .`

After ANSSI's LIBS-AUDIT and LIBS-OUTDATED, with tools from the Rust Secure Code WG and RustSec. It needs the network.

- `cargo deny check`: RustSec advisories (any vulnerability fails), licenses against the allow-list in `deny.toml`,
  crates.io as the only source, no wildcard versions. Duplicate versions are reported, not failed.
- `cargo outdated --root-deps-only --exit-code 1`: a direct dependency with a newer release fails the stage, until it is
  updated or the choice is justified in `deny.toml` / [dependencies](dependencies.md).
- `cargo audit bin`: the release binary, which `cargo auditable` builds with its dependency tree inside.

Last run (2026-10-23): advisories, bans, licenses and sources ok. All direct dependencies are up to date (after updating
`getrandom` to 0.4). The binary carries 277 dependencies, with no advisories beyond the accepted `paste` one (see [dependencies](dependencies.md)).

## Self-hosted, by agents: the README's round trip (2026-10-27)

Two fresh Claude Haiku 4.5 sessions, Ada and Bob, each in an empty directory with its own volumes and container
names, and deny rules keeping them off every other volume, Datum and `~/.claude/skills`. Both followed
[self-hosting](self-hosting.md) from the chaosgejo raw URL, with the current image (`sha256:b9845cb8`), and both
machines reached Datum's relays from this workstation.

- **Set-up, in parallel:** each ran `owner init`, `owner server-env`, started its server, saw `online: reachable
  through` and passed GUIDE.md's self-test. Ada also made an invite for Bob (Ada 70 s, Bob 62 s; $0.11 each). Neither
  reported a problem with the guides.
- **Bob** left "Lunch Friday?" with `--reply-invite auto` from the pasted invite message.
- **Ada** read it (`can reply: yes`) and replied with `owner reply`; **Bob** read the reply, under the label
  "Ada's assistant (replying to you)", itself answerable.
- **A stranger** (a fresh owner key, run by hand) was refused at Ada's owner door by `owner connect`, exit 77.
- **Cleanup:** each agent removed its container, volumes and settings file; independently confirmed gone, with the
  owner's own volumes untouched. Eight sessions, $1.02 in all.

The README's example is this run's output, checked against each volume's `owner history`. Messages are #2 because each
machine's self-test took #1.

## On Datum, by hand and by agents (2026-10-23 and 24, project-t96fh, us-west-1)

The current image (`sha256:b9845cb8`, on Red Hat's hardened `hi/static` base, running as uid 65532, with rusqlite 0.41)
passed `check` and `audit`, and a smoke test through Datum's relays from local containers, with fresh volumes removed
afterwards: invite, leave with `--reply-invite auto`, inbox, `owner reply` arriving on the sender's side, a revoked
invite refused (exit 77) and a stopped server (exit 69). It has not been deployed to Datum Compute. Its predecessor
(`sha256:4442ff25`, the same code on rusqlite 0.40) was, from GUIDE.md's commands, and torn down:
- `Available` in about 20 s, and on a relay 3 s after start
- `owner status`, then a message left and fetched, which shows the non-root server can write its SQLite mailbox
- every resource confirmed `NotFound` afterwards

Then fresh Claude Haiku 4.5 sessions ran it end to end from the repository's guides alone:
- **Owner:** GUIDE.md through the self-test and an invite, with every check passing and no guide problems reported
  (106 s, $0.15).
- **Sender:** given only the pasted invite message, followed LEAVE.md and delivered (30 s, $0.05).
- **Owner again:** read the message and tore everything down (46 s, $0.06). All three resources were independently
  confirmed `NotFound`.
- One slip, in the agent rather than the guide: on its second session, the owner used the guide's default volume name
  instead of the one it was told to use. A deny rule blocked it as intended, and one reminder fixed it.

Before that, it and its predecessors were smoke-tested through Datum's relays, from local containers:
- leave with `--reply-invite auto`, inbox, and a threaded `owner reply`
- revoke and add-owner through a manifest re-apply: the revoked invite was refused, and the added owner connected
- exit codes: 0 delivered, 77 not an owner, 69 with the server stopped (after the 30 s dial timeout)
- self-hosted from `owner server-env`, with the mailbox on a volume: a message survived a restart

With the earlier images:

- GUIDE.md followed by hand, with the image pulled anonymously:
  - `Available` in under a minute, and on a relay 3 s after start.
  - `owner status` in about 1 s; the self-test leave and inbox worked.
  - Teardown left nothing.
- `datumctl compute restart` kept the endpoint id, and invites made before it still worked. Unread messages were lost, as
  designed.
- Fresh agent sessions provisioned from GUIDE.md and tore down again: Claude Haiku, Codex (gpt-5.6-luna) and Sonnet.
  - They left messages through LEAVE.md, replied with poems through `--reply-invite auto` and `owner reply`, and answered
    a scheduling question.
  - Each cleanup was checked independently.
- Fixed from those runs:
  - `--http-proxy=false` now leads the `datumctl` command.
  - The wait for `Available` is a bounded loop.
  - The self-test comes before the first invite.
  - `am skill` runs without the state volume.
  - Each deployment gets its own self-test volume.
- Not verified: Docker (only podman), Gemini CLI, more than two owners, a real flood against the limits.
