# AGENTS.md

**Read [docs/agents.md](docs/agents.md) before changing anything.** It has the layout, the invariants and the tests that
enforce them, the security checklist, and how to build, release and test against Datum.

If you were asked to *use* an answering machine rather than work on this code, follow [GUIDE.md](GUIDE.md) (set one up)
or [LEAVE.md](LEAVE.md) (leave a message), and **do not build the image or clone anything**: pull the pinned
`ghcr.io/dlotterman/iroh-answering-machine@sha256:…` those guides name. Set-up goes on Datum; if you lack an account or a
login, ask your human for it before considering self-hosting. The build instructions below are for changing this code only.

## Tools

- **Containers:** this repository uses rootless Podman, but nothing depends on it: Docker or any compatible tool works
  the same way (swap the command, and leave out Podman-only flags such as `--http-proxy=false`). With Podman, use it
  rootless. On SELinux systems (Fedora, RHEL, Alma, Rocky), add `:z` to bind mounts. For
  rootless options such as `--userns=keep-id`, see the
  [rootless tutorial](https://github.com/podman-container-tools/podman/blob/main/docs/tutorials/rootless_tutorial.md).
- **datumctl:** if it isn't installed, run it from the
  [datumctl container](https://hub.docker.com/r/dlotterman/datumctl-container) with the login kept in a volume:
  `podman run --rm -i --http-proxy=false -v datumctl-config:/root/.datumctl docker.io/dlotterman/datumctl-container:latest <command>`.
  Keep `-i` (and no `-t`) so a manifest can be piped into `apply -f -`. Docs:
  [datumctl](https://www.datum.net/docs/datumctl/overview), [Datum for agents](https://www.datum.net/docs/agents/llms-txt).
- **Rust (only when changing this code):** no toolchain on the host is needed or assumed; everything builds in the Containerfile.
