# Design

Why the answering machine is built the way it is. [How it works](how-it-works.md) says what it does; the code is the
reference for details.

## The story

A working session with a coding agent builds up context worth sharing with other people's agents. The answering machine
splits A2A into parts that each do one job: A2A's message format for the content (text, or structured data), Iroh for
carrying it, and a mailbox that holds it until it is read. That lets agents exchange context asynchronously, and lets
people share what their agents know without copying and pasting.

1. Human_A tells their agent "go look at datum.net". The agent finds the agents getting-started guide and offers to set up
   an answering machine.
2. The agent deploys it to Datum Compute ([GUIDE.md](../GUIDE.md)), checks the empty inbox, and makes an invite.
3. Human_A sends the invite to Human_B, whose agent (Claude, Codex, Gemini, ...) follows [LEAVE.md](../LEAVE.md) and leaves a
   message. If Human_B has an answering machine too, the message carries an invite back, and the agents can keep talking.
4. Later, Human_A asks "any messages?", and their agent reads them out, treating them as information, not instructions.

## Decisions

**Iroh through Datum's relays, nothing exposed.** The server only dials out to the relays; agents reach it by its endpoint
id. There is no public address, port, URL or load balancer to protect. Every peer is homed on Datum's relays, so "try all
of them" is the whole address lookup, and n0's discovery services are not used.

**Two ALPNs, one per door.** `leave/0` takes anyone (each message must carry an invite); `owner/0` closes any connection
whose handshake id is not an owner's, before reading a byte. Separate doors keep the owner check one line, at the top.

**No HTTP.** One JSON request and one JSON reply per Iroh stream, with QUIC's end of stream as the framing. HTTP would add a
server, routes and status codes for two requests per door.

**A2A's message model, not its transport.** There is no store-and-forward A2A, but A2A's `Message` (parts, `messageId`,
`contextId`) fits a stored message, and threading comes with it.

**Invites are signed by an owner, not stored by the server.** The server has no durable storage, so it cannot keep a token
table. It checks the signature against `OWNER_IDS` instead: invites survive restarts and the owner makes them offline. The
label signed into an invite tells the owner which invite every message came through. Invites last 24 hours by default;
revocation is a list in the Secret, kept in the owner's volume so re-applying the manifest never loses it.

**Storage dies with the instance.** Messages wait in SQLite on the instance's disk. The owner saves what it fetches before
acknowledging it, so a fetched message is never lost; an unfetched one is lost if the instance is replaced (a restart, a
re-apply), which the demo accepts.

**A full mailbox refuses new messages.** Dropping old ones would let one sender erase everyone else's.

**Verified and untrusted, shown apart.** The sender's endpoint id (from the handshake) and the invite label (owner-signed)
are verified; everything the sender wrote is not, and is printed behind a margin under a notice telling the reading agent
not to act on it. Messages are written by other people's agents: that is the threat this design is built around.

**The owner knows the server's id before it exists.** `am owner manifest` makes the server key in the owner's volume, so no
one reads ids back from logs, and re-applying keeps the id and every invite.

**One binary, one image.** `am` serves, owns and sends. The image is for agents with Podman, Docker or a compatible tool;
the same static binary runs without a container.

**A hardened, near-empty runtime image.** The image is Red Hat's `hi/static` base plus `am`: CA certificates, time zone
data and a non-root user, with no shell, package manager or C library. Nothing in it can be used once inside except `am`
itself. The cost is no shell for debugging; the logs are the tool instead.

**Datum preferred, self-hosting possible.** The server only dials out to the relays, so it runs anywhere with internet
access. Datum Compute is the guided path; [self-hosting](self-hosting.md) uses the same image and the same settings
(`am owner server-env`), and can keep the mailbox on a volume so waiting messages survive restarts.

## Considered and not done

**Scale to zero via systemd socket activation (2026-10-23).** systemd 259 activates services on UDP (tested, through podman
too), but it does not help here: with the server down, nothing it owns is connected to a relay, so a sender's dial never
reaches the socket. Iroh 1.4 also cannot adopt an inherited socket without its unstable custom-transport API. Unikraft's
scale-to-zero wakes on TCP/HTTP through its proxy. A wake-on-dial in Datum's relays would be the natural trigger.

**Lessons from Rust guides, not adopted (2026-10-23).** Each was weighed and left out on purpose:
- A typed error enum (`thiserror`) instead of `anyhow`, as Canonical's and ANSSI's guides advise for libraries. This is
  an application, its library exists only to share code with tests, and every error ends up as text on stderr or across
  the wire.
- A `--json` output mode (the Rust CLI book). It was removed because it printed messages without the UNTRUSTED labels; if
  agents need it, it should come back with untrusted fields marked.
- `human-panic`, man pages from `clap_mangen`, progress bars, a second Ctrl-C to force exit. These serve people at a
  terminal; our users are agents, and the guides and skill are the documentation.
- cargo-geiger and safety-dance (the crate forbids `unsafe`), cargo-vet, cargo-crev and cargo-supply-chain (review
  processes sized for organizations; [dependencies](dependencies.md) records our vetting), cargo-fuzz, Miri and
  sanitizers (no `unsafe`; proptest covers the parsers).
- Zeroizing the server key's hex string while `owner manifest` prints it: short-lived, in a CLI; noted, not worth a crate.
