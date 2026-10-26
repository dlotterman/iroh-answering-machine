# Dependencies

Each direct dependency, why it is here, and how it was vetted (ANSSI LIBS-VETTING-DIRECT: validate each one and keep the
record). "Blessed" means [blessed.rs](https://blessed.rs/crates) recommends it for this job. `podman build --target audit .`
checks the whole tree (about 400 crates) against RustSec advisories, the license list and the allowed sources in
`deny.toml`, and that no direct dependency is outdated.

| Crate | For | License | Vetting |
| --- | --- | --- | --- |
| `iroh` | QUIC endpoints, relays, keys and signatures | MIT / Apache-2.0 | n0's networking stack, used by Datum's own relays |
| `n0-future` | The stream types iroh's address-lookup trait uses | MIT / Apache-2.0 | iroh's companion crate |
| `tokio` | Async runtime | MIT | Blessed |
| `rusqlite` (bundled SQLite) | The server's mailbox | MIT (SQLite: public domain) | Blessed |
| `serde`, `serde_json` | JSON on the wire and on disk | MIT / Apache-2.0 | Blessed |
| `postcard` | Compact encoding of signed invites | MIT / Apache-2.0 | Blessed |
| `data-encoding` | Hex and base32 for keys and invites | MIT | Widely used; no unsafe in our use |
| `getrandom` | Message ids and invite ids | MIT / Apache-2.0 | The OS random source behind `rand` |
| `clap` | The command line | MIT / Apache-2.0 | Blessed |
| `anyhow` | Errors in an application | MIT / Apache-2.0 | Blessed (applications) |
| `tracing`, `tracing-subscriber` | Server logs | MIT | Blessed |
| `tempfile`, `assert_cmd`, `predicates`, `proptest` (tests only) | Temporary state, running the binary, property tests | MIT / Apache-2.0 | Blessed; Rust CLI book; Rust Secure Code WG list |

Known and accepted: `paste` (RUSTSEC-2024-0436, unmaintained) arrives through iroh's `netwatch` as a compile-time macro;
nothing of it runs, and only iroh can drop it. Recheck when upgrading iroh.

The released binary is built with `cargo auditable`, so the dependency list travels inside it: `cargo audit bin am`
checks a deployed image against new advisories without this repository.

## Licenses

The whole Rust tree (about 400 crates, `cargo deny list`) is under permissive licenses, which `deny.toml` allows and
nothing else: Apache-2.0 (also with the LLVM exception), MIT, MIT-0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, BSL-1.0,
CC0-1.0, Unicode-3.0, CDLA-Permissive-2.0 (webpki-roots, Mozilla's CA list as data) and the Unlicense.

The container image is Red Hat's hardened `hi/static` base (pinned by digest in the Containerfile) plus the static `am`
binary. The base holds data rather than programs: no shell, no package manager, no C library. It runs as a non-root
user (65532). Its packages keep their own licenses:

| Package | License |
| --- | --- |
| ca-certificates-bundle | MIT and GPL-2.0-or-later |
| tzdata | public domain, and GPL-2.0-only with the Classpath exception |
| filesystem, setup | public domain |
| hummingbird-release, hummingbird-repos, hummingbird-gpg-keys | GPL-2.0-or-later |

`am` does not link any of them.

Red Hat also publishes a hardened Rust image (`hi/rust`). It is not used to build: it has no musl target and no
`rustup` or package manager to add one, so it cannot build the static binary that lets the runtime image stay this
small. The build stages stay on the official `rust` image, pinned.
