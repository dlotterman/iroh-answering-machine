# Commands

`am` is one program for every role. In the container, run it as:

```sh
podman run --rm -i --http-proxy=false -v answering-machine:/state ghcr.io/dlotterman/iroh-answering-machine@sha256:… <command>
```

The guides pin the exact image.

> **A note on containers:** the commands here use `podman` (rootless), because that is how this project is built
> and tested. Docker, or any other compatible container tool, works the same way: swap the command name and leave
> out Podman-only flags such as `--http-proxy=false`. So far only Podman has been tested.

## Owner: your answering machine

| Command | What it does |
| --- | --- |
| `am owner init --name NAME` | Make the owner key (once) and set the name senders see. Prints `OWNER_ID=`. |
| `am owner id` | Print `OWNER_ID=` again. |
| `am owner manifest --name NAME [--image IMAGE]` | Print the Datum manifest for `datumctl apply -f -`. Makes the server key the first time; prints `SERVER_ENDPOINT_ID=` on stderr. The output holds the server's secret key: pipe it, never print it. |
| `am owner server-env` | Print the server's settings as an env file, to [run it yourself](self-hosting.md) instead of on Datum. Same key and settings as `manifest`; prints `SERVER_ENDPOINT_ID=` on stderr. Secret: write it to a mode-600 file. |
| `am owner status` | The answering machine's endpoint id and how many messages are waiting. |
| `am owner inbox` | Fetch new messages, save them locally, delete them from the server, and print them. |
| `am owner history [--last N]` | Print saved messages. |
| `am owner reply N --text TEXT` | Answer message N through the reply invite its sender attached. |
| `am owner invite --label LABEL [--hours H] [--quiet]` | Make an invite (24 hours by default, 720 at most) and a message to send with it. |
| `am owner invites` | Invites made here: id, valid / expired / revoked, label. |
| `am owner revoke ID` | Refuse an invite before it expires. |
| `am owner add-owner ID`, `remove-owner ID`, `owners` | Other owner identities (another machine or agent of the same person). |
| `am owner connect ID` | On an extra owner's machine: remember which answering machine to use. |

`revoke`, `add-owner` and `remove-owner` take effect when the manifest is applied again and the workload restarted (see
GUIDE.md, "Apply a change"), or, self-hosted, when the env file is written again and the container recreated. A restart loses messages nobody has fetched, so run `am owner inbox` first.

## Sender: leaving messages

| Command | What it does |
| --- | --- |
| `am invites save NAME` | Save an invite under a short name. The invite is read from stdin, so it stays out of command lines. |
| `am invites list` | Saved invites, and whether they have expired. |
| `am invites forget NAME` | Delete a saved invite. |
| `am leave NAME_OR_INVITE --text TEXT [--from WHO] [--reply-invite auto]` | Leave a message. `--text -` reads it from stdin. `--reply-invite auto` attaches a fresh invite to your own answering machine. |
| `am whoami` | The endpoint id your messages come from. |

## Server

| Command | What it does |
| --- | --- |
| `am serve` | Run the answering machine. Datum runs it for you, or you [run it yourself](self-hosting.md). |

It reads its settings from the environment, which the manifest's Secret provides:

| Variable | Meaning |
| --- | --- |
| `SERVER_SECRET_KEY_HEX` | The server's key; fixes its endpoint id. Required. |
| `OWNER_IDS` | Owner endpoint ids, comma-separated. At least one. |
| `REVOKED_INVITES` | Invite ids to refuse, comma-separated. |
| `AM_DB` | The SQLite file. Default `/var/lib/answering-machine/inbox.db`. |
| `RELAYS` | Relay URLs, comma-separated. Default: Datum's three relays. |

## Anyone

| Command | What it does |
| --- | --- |
| `am skill [--image IMAGE]` | Print the agent skill (SKILL.md) with the image filled in. |
| `am --help`, `am <command> --help` | Help. |

## Errors and exit codes

Errors go to stderr and read as one sentence: `error: cannot <what you asked for>: <why>`.

| Exit code | Meaning |
| --- | --- |
| 0 | Done. |
| 1 | Failed here: a bad or expired invite, a missing owner identity, a typo. |
| 2 | Bad command-line arguments. |
| 69 | The answering machine could not be reached. Try again later. |
| 77 | The answering machine refused: a bad invite, not an owner, the mailbox is full, too many messages. |

69 and 77 follow BSD `sysexits`, so a script can tell "down" from "no" without reading the message.

## Files

Everything lives in the state directory: `/state` in the container (a named volume), or `AM_STATE`.

| File | What |
| --- | --- |
| `owner.key` | The owner key. Secret. |
| `owner.json` | Your name, your answering machine's id, extra owners, revoked invites. |
| `server.key` | The answering machine's key, made by `owner manifest` or `owner server-env`. Secret. |
| `messages.jsonl` | Every message you have fetched. The server forgets them once fetched; this does not. |
| `issued.jsonl` | Invites you made: ids, labels, expiry. Not the invites themselves. |
| `sender.key` | The key you leave messages with, kept apart from the owner key. Secret. |
| `invites/NAME` | Invites to other people's answering machines. Secret. |
