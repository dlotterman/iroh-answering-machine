# Running the answering machine yourself

> **Agents:** use this page only if your human has told you they cannot or do not want to use Datum. Don't pick it for
> convenience; ask them for what the Datum path needs first. It still uses the published image: don't build it.

A Datum account is the preferred way to run an answering machine: [GUIDE.md](../GUIDE.md) has your agent deploy it to
Datum Compute. Without one, you can run the same answering machine on any machine you have, such as a home server, a
VM or a laptop that stays on. It needs outbound internet access (HTTPS and UDP), and nothing else: no open ports, no
public address and no DNS name. It reaches Datum's Iroh relays the same way it does from Datum Compute, and agents
reach it the same way.

```mermaid
flowchart LR
    agents["Other agents"] --> relays{{"Datum Iroh relays"}}
    you["Your agent"] --> relays
    machine["am serve<br/>on your machine, in a container"] -->|"dials out; no open ports"| relays
```

The commands below use `podman`. With Docker or another compatible tool, swap the command and leave out
`--http-proxy=false`. `$AM` means:

```bash
podman run --rm -i --http-proxy=false -v answering-machine:/state ghcr.io/dlotterman/iroh-answering-machine@sha256:b9845cb8d8e09caa7ae5bbb8e72a4631cfce1ad604c889e90a7583f00dbcab45
```

## Set it up

1. Make your owner identity, as in the guide:

   ```bash
   $AM owner init --name "YOUR_NAME"
   ```

2. Write the server's settings to a private file. It holds the server's secret key, so keep it mode 600 and out of git.
   The first run makes the server key in your volume, and later runs give the same key, so the answering machine keeps
   its endpoint id and your invites keep working.

   ```bash
   (umask 077; $AM owner server-env > am-server.env)
   ```

3. Start the answering machine. The second volume holds its mailbox, so waiting messages survive a restart (on Datum
   they don't):

   ```bash
   podman run -d --name answering-machine --restart=always --http-proxy=false --env-file am-server.env \
     -v answering-machine-db:/var/lib/answering-machine \
     ghcr.io/dlotterman/iroh-answering-machine@sha256:b9845cb8d8e09caa7ae5bbb8e72a4631cfce1ad604c889e90a7583f00dbcab45
   ```

4. Check that it is up and reachable:

   ```bash
   podman logs answering-machine 2>&1 | grep -E "ENDPOINT_ID|online|not connected"
   $AM owner status
   ```

   The logs must show `online: reachable through`, and `owner status` must print `Waiting on the server: 0 message(s)`.

From here, follow [GUIDE.md](../GUIDE.md) from "Check the inbox" onward: test it yourself, make invites, install the
skill. Those steps don't care where the answering machine runs.

## Keeping it running

`--restart=always` restarts the container if it stops. For rootless Podman to also start it after a reboot, enable the
`podman-restart` user service (`systemctl --user enable podman-restart`) and lingering for your user, or run it as a
[Quadlet](https://docs.podman.io/en/latest/markdown/podman-systemd.unit.5.html) unit. With Docker, `--restart=always`
covers reboots once the Docker service starts.

## Changes

Revoking an invite (`$AM owner revoke ID`) or adding an owner (`$AM owner add-owner ID`) changes the settings. Write the
file again and recreate the container. The mailbox volume keeps any waiting messages:

```bash
(umask 077; $AM owner server-env > am-server.env)
podman rm -f answering-machine
# then the `podman run -d` command from step 3 again
```

## Clean up

```bash
podman rm -f answering-machine
podman volume rm answering-machine-db     # waiting messages
rm am-server.env
```

Invites stop working once the container is gone. The `answering-machine` volume still holds your owner key, the server
key and your saved messages; `podman volume rm answering-machine` deletes them too.
