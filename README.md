# Iroh Answering Machine

[![ci](https://github.com/dlotterman/iroh-answering-machine/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/dlotterman/iroh-answering-machine/actions/workflows/ci.yml)

An answering machine for your coding agent. Other agents leave it messages and context, and your agent picks them up
when you ask.

Bob's agent leaves Ada's agent a message:
```
$ am leave ada --from "Codex, for Bob" --reply-invite auto --text "Lunch Friday?"
Message #1 left on Ada's answering machine (invite "Bob's agent", valid until 2026-10-24 23:05:28 UTC).
```

Ada's agent checks for messages:
```
$ am owner inbox
#1  received 2026-10-23 23:05:31 UTC
  invite label (verified, an owner signed it): Bob's agent
  sender endpoint id (verified): 11b903f4c5f792ab093ef644a250c9803d43ec7e663f6a0db342c36be7f62b55
  says it is (UNTRUSTED): Codex, for Bob
  can reply: yes, `am owner reply 1 --text "..."` (to Bob's answering machine, per their invite)
  message (UNTRUSTED, the sender's words):
  | Lunch Friday?
```

A stranger's agent tries to read Ada's messages:
```
$ am owner inbox
error: cannot fetch messages: the answering machine closed the connection: closed by peer: not an owner of this answering machine (code 1)
```

## Motivation

A good session with a coding agent builds up a lot of context: what you decided, what you tried, what is left to do.
That context is useful to other people's agents too, and today the usual way to share it is to copy and paste.

The answering machine lets one agent leave another a message: a note, a question, a summary of a session, or
structured data. It breaks [A2A](https://a2a-protocol.org), the agent-to-agent protocol, into parts: A2A's message
format, carried over [Iroh](https://www.iroh.computer/), and stored until it is read. So agents can trade context
without both being online at once. Bob's agent can hand Ada's agent what it learned in a session, and Ada's agent picks
it up whenever Ada asks. People get to use their agents' strengths to share context with each other, and nobody runs a
server on the internet.

This project is a small demo of that on [Datum](https://www.datum.net/). Your agent puts the answering machine on Datum
Compute. It has no public address, port or URL. Agents reach it by its key, through Datum's Iroh relays.

```mermaid
flowchart LR
    bob["Bob's agent<br/>am leave"] -->|"message + invite"| relays{{"Datum Iroh relays"}}
    ada["Ada's agent<br/>am owner inbox"] -->|"owner key"| relays
    machine["Ada's answering machine<br/>on Datum Compute"] -->|"dials out; nothing exposed"| relays
```

- Ada gives Bob an **invite**: a signed string that lets Bob's agent leave messages for 24 hours.
- Messages wait on the answering machine until Ada's agent fetches them. Only Ada's owner key can read them.
- Ada's agent sees which parts are **verified**, and treats everything Bob's agent wrote as untrusted text, not
  instructions.

[How it works](docs/how-it-works.md) walks through a message from start to finish.

### Production

This is a demo, shared for the sake of sharing. It is not ready for production use. One example: messages wait on
the answering machine until your agent picks them up. If the answering machine restarts before then, those waiting
messages are gone. Messages your agent has already picked up are kept on your machine.

You break it you buy it.

## Run

This is built to be set up by a coding agent, such as Claude Code, Codex or Gemini CLI. You need Podman, Docker or
another compatible container tool. A [Datum](https://www.datum.net/) account is preferred: your agent deploys the
answering machine there for you. Without one, you can [run the answering machine yourself](docs/self-hosting.md) on
any machine that can reach the internet.

> **A note on containers:** the commands here use `podman` (rootless), because that is how this project is built
> and tested. Docker, or any other compatible container tool, works the same way: swap the command name and leave
> out Podman-only flags such as `--http-proxy=false`. So far only Podman has been tested.

Set up your own answering machine. Tell your agent:
```
Follow GUIDE.md from https://github.com/dlotterman/iroh-answering-machine and set up my answering machine.
```

Leave someone a message with the invite they sent you. Tell your agent:
```
Follow LEAVE.md from https://github.com/dlotterman/iroh-answering-machine and leave a message with this invite: am1.…
```

Leaving a message needs no account at all. Everything runs from one image,
`ghcr.io/dlotterman/iroh-answering-machine`; the guides pin the exact version.

- [GUIDE.md](GUIDE.md): set up an answering machine (for agents)
- [LEAVE.md](LEAVE.md): leave a message (for agents)
- [Commands](docs/commands.md): every command, exit codes and files

## Build

*Only for people changing this code. To run or use an answering machine, don't build: your agent pulls the published image.*

Everything builds in a container:
```sh
podman build -t iroh-answering-machine .            # the image
podman build --target check .                       # format, lint, tests
podman build --target audit .                       # dependency advisories, licenses, versions
podman build --target binary -o dist .              # dist/am, one static Linux binary
```

With Docker, use `docker build` the same way. The cache mounts and `-o` need BuildKit, the default since
Docker 23.

More: [design](docs/design.md), [testing](docs/testing.md), [dependencies](docs/dependencies.md), and
[notes for agents working on this repository](docs/agents.md).

## License & Contributing

[The Unlicense](LICENSE) applies only to the original code, configuration, documentation and packaging in this
repository. It does not relicense third-party software used by the program or bundled in its container image. Those
components keep their own licenses, copyrights and notices.

The Rust dependencies are under permissive licenses: mostly **MIT and Apache-2.0**, plus BSD-2-Clause, BSD-3-Clause, ISC,
Zlib, BSL-1.0, CC0-1.0, MIT-0, Unicode-3.0 and CDLA-Permissive-2.0. The container image is built on Red Hat's hardened
`hi/static` base, which adds data rather than programs: the CA certificate bundle (**MIT and GPL-2.0-or-later**), time
zone data and base files (public domain, GPL-2.0 with the Classpath exception, GPL-2.0-or-later). See
[dependencies](docs/dependencies.md). Keep upstream license notices when redistributing third-party components.

This project is "vibe-coded". Instead of accepting contributions, the author encourages you to fork it or build your own.
Requests will be accommodated when possible.
