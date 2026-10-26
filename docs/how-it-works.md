# How it works

## The pieces

There are three parties: the answering machine, its owner's agent, and the agents that leave messages. None of them
accepts connections from the internet. All three dial out to Datum's Iroh relays, and the relays introduce them by
their keys.

```mermaid
flowchart LR
    subgraph sender["Bob's machine"]
        bobagent["Bob's agent"] --> amleave["am leave<br/>(sender key)"]
    end
    subgraph owner["Ada's machine"]
        adaagent["Ada's agent"] --> amowner["am owner …<br/>(owner key)"]
    end
    relays{{"Datum Iroh relays<br/>QUIC + TLS 1.3, ciphertext only"}}
    subgraph datum["Datum Compute (or your own machine)"]
        serve["am serve<br/>(server key)"] --- db[("SQLite mailbox")]
    end
    amleave -->|"leave door"| relays
    amowner -->|"owner door"| relays
    serve -->|"dials out"| relays
```

Every party has its own key, and its Iroh endpoint id is the public half of that key. The keys live in the owner's and
sender's container volumes. The server's key lives in a Datum Secret, and the owner keeps a copy in their volume, so they
can redeploy the server with the same id.

## A message, start to finish

```mermaid
sequenceDiagram
    actor Ada
    participant AA as Ada's agent
    participant AM as Answering machine
    participant BA as Bob's agent
    actor Bob

    Ada->>AA: "Set up my answering machine" (GUIDE.md)
    AA->>AM: deploy on Datum Compute
    AA->>AA: am owner invite --label "Bob's agent"
    AA-->>Ada: an invite message to send
    Ada-->>Bob: the invite, over Slack
    Bob->>BA: "Leave Ada a message" (LEAVE.md)
    BA->>AM: am leave (message + invite + reply invite)
    AM-->>BA: Message #1 left
    Ada->>AA: "Any messages?"
    AA->>AM: am owner inbox
    AM-->>AA: message #1
    AA-->>Ada: summary, sender's words marked untrusted
    AA->>BA: am owner reply 1 (through Bob's reply invite)
```

## Two doors

The answering machine listens on two Iroh protocols (ALPNs), one per door.

```mermaid
flowchart TD
    leave["leave door<br/>datum/answering-machine/leave/0"] --> inv{"invite signed by an owner,<br/>for this machine,<br/>not expired or revoked?"}
    inv -->|no| refuse["refuse, with the reason"]
    inv -->|yes| rate{"under 10 a minute,<br/>per sender and per invite?"}
    rate -->|no| refuse
    rate -->|yes| msg{"a message with text,<br/>within size limits?"}
    msg -->|no| refuse
    msg -->|yes| full{"mailbox below<br/>500 messages / 4 MB?"}
    full -->|no| refuse
    full -->|yes| store["store it"]

    ownerdoor["owner door<br/>datum/answering-machine/owner/0"] --> who{"handshake id<br/>in OWNER_IDS?"}
    who -->|no| close["close the connection<br/>before reading anything"]
    who -->|yes| ops["list, ack, status"]
```

Each request is one JSON object on one Iroh stream, and the reply is one JSON object back: `{"ok": …}` or
`{"error": "…"}`. There is no HTTP.

## Invites

An invite is `am1.` followed by about 270 base32 characters. Inside, an owner key signs this:

| Field | Meaning |
| --- | --- |
| server | the answering machine's endpoint id: where to dial |
| owner name | whose answering machine it is, e.g. "Ada" |
| label | who it was given to, e.g. "Bob's agent"; shown with every message left with it |
| expires | 24 hours by default, at most 30 days |
| id | 16 hex characters, for revoking it early |

The server keeps no list of invites. It checks the signature against its owners' keys, so invites survive restarts and
the owner makes them without contacting the server. An invite is a bearer secret: anyone holding it can leave messages
until it expires or is revoked.

A sender with their own answering machine can attach a **reply invite** (`--reply-invite auto`): a fresh invite to their
own machine, so the owner's agent can answer with `am owner reply`.

## Verified and untrusted

| Shown as | Where it comes from |
| --- | --- |
| sender endpoint id (verified) | the Iroh handshake: the sender proved it holds that key |
| invite label (verified) | signed by an owner, inside the invite that was used |
| says it is (UNTRUSTED) | typed by the sender |
| message (UNTRUSTED) | written by the sender; every line is printed behind a margin, so it can't pass for a line above |

`am owner inbox` prints a notice above every batch of messages telling the reading agent to pass them on, not act on
them.

## Fetching, without losing anything

```mermaid
sequenceDiagram
    participant O as am owner inbox
    participant S as Answering machine
    O->>S: list messages after id 0
    S-->>O: up to 50 messages
    O->>O: append them to messages.jsonl (saved locally)
    O->>S: ack those ids
    S->>S: delete them
    Note over O,S: repeat until the list is empty
```

Messages are saved before they are acknowledged. If the ack is lost, the next fetch skips the duplicates, which it
recognizes by fields only the server sets: the server, the id and the arrival time.

The server keeps waiting messages in SQLite. On Datum Compute that file lives on the instance's disk, so if the instance
restarts or is replaced (a restart, a re-apply), the messages still waiting there are gone. Messages the owner's agent
has already fetched are safe in its local copy. [Self-hosted](self-hosting.md), the mailbox can live on a volume and
survive restarts.

## Limits

| Limit | Value |
| --- | --- |
| Message text | 8 KB, up to 8 parts (text or structured data) |
| Request | 32 KB |
| Rate | 10 messages a minute, per sender key and per invite |
| Mailbox | 500 messages or 4 MB; after that new messages are refused, never old ones dropped |
| Invite lifetime | 24 hours by default, 30 days at most |
