//! The `am` binary from the outside: the output contract agents and the guides rely on (`OWNER_ID=`, the manifest on
//! stdout and the server id on stderr, `Saved "…"`, the wording of refusals), and the exit codes.
//!
//! After the Rust CLI book's testing chapter (https://rust-cli.github.io/book/tutorial/testing.html): run the real binary
//! with `assert_cmd`, check that the key strings are there rather than whole layouts, and leave `--help` alone. Nothing
//! here needs the network; `RELAYS` points at a dead local address so nothing tries.
// Tests may unwrap (see clippy.toml); clippy's exemption covers #[test] functions but not their helpers.
#![allow(clippy::unwrap_used)]
use std::path::Path;

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use predicates::prelude::*;
use tempfile::TempDir;

/// `am` with its state in `state` and no way to reach real relays.
fn am(state: &Path) -> Command {
    let mut command = cargo_bin_cmd!("am");
    command.env("AM_STATE", state).env("RELAYS", "http://127.0.0.1:9").env_remove("RUST_LOG");
    command
}

fn stdout_of(command: &mut Command) -> String {
    String::from_utf8(command.assert().success().get_output().stdout.clone()).unwrap()
}

/// An owner with a manifest made, so the volume knows its server: what the guide has done before inviting anyone.
fn deployed_owner() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    am(dir.path()).args(["owner", "init", "--name", "Ada"]).assert().success();
    am(dir.path()).args(["owner", "manifest", "--name", "am-test1", "--image", "img"]).assert().success();
    dir
}

fn make_invite(owner: &Path, extra: &[&str]) -> String {
    let mut command = am(owner);
    command.args(["owner", "invite", "--label", "Bob's agent", "--quiet"]).args(extra);
    stdout_of(&mut command).trim().to_string()
}

#[test]
fn owner_init_prints_an_owner_id_that_stays_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let first = stdout_of(am(dir.path()).args(["owner", "init", "--name", "Ada"]));
    assert!(predicate::str::is_match("^OWNER_ID=[0-9a-f]{64}\n$").unwrap().eval(&first), "got {first:?}");
    let again = stdout_of(am(dir.path()).args(["owner", "init", "--name", "Ada L."]));
    assert_eq!(first, again, "re-running init keeps the key");
    assert_eq!(first, stdout_of(am(dir.path()).args(["owner", "id"])));
}

#[test]
fn owner_commands_before_init_say_how_to_start() {
    let dir = tempfile::tempdir().unwrap();
    am(dir.path())
        .args(["owner", "invite", "--label", "Bob's agent"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("am owner init"));
}

#[test]
fn the_manifest_goes_to_stdout_and_the_server_id_to_stderr() {
    let dir = tempfile::tempdir().unwrap();
    am(dir.path()).args(["owner", "init", "--name", "Ada"]).assert().success();
    am(dir.path())
        .args(["owner", "manifest", "--name", "am-test1", "--image", "img"])
        .assert()
        .success()
        .stdout(predicate::str::contains("kind: Workload").and(predicate::str::contains("__").not()))
        .stdout(predicate::str::contains("SERVER_ENDPOINT_ID").not())
        .stderr(predicate::str::is_match("SERVER_ENDPOINT_ID=[0-9a-f]{64}").unwrap());
    am(dir.path()).args(["owner", "manifest", "--name", "Not_A_Name"]).assert().code(1);
}

#[test]
fn an_invite_travels_from_owner_to_sender_through_stdin() {
    let owner = deployed_owner();
    let sender = tempfile::tempdir().unwrap();
    let invite = make_invite(owner.path(), &[]);
    am(sender.path())
        .args(["invites", "save", "ada"])
        .write_stdin(invite)
        .assert()
        .success()
        .stdout(predicate::str::contains("Saved \"ada\": an invite to Ada's answering machine"))
        .stderr(predicate::str::contains("Ctrl-D").not()); // piped stdin: no hint meant for a person at a terminal
    am(sender.path()).args(["invites", "list"]).assert().success().stdout(predicate::str::starts_with("ada: Ada's"));
}

#[test]
fn an_expired_invite_is_refused_before_anything_is_sent() {
    let owner = deployed_owner();
    let sender = tempfile::tempdir().unwrap();
    let expired = make_invite(owner.path(), &["--hours", "0"]);
    am(sender.path())
        .args(["invites", "save", "old"])
        .write_stdin(expired.clone())
        .assert()
        .code(1)
        .stderr(predicate::str::contains("expired"));
    am(sender.path())
        .args(["leave", &expired, "--text", "hi"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("expired"));
}

#[test]
fn a_revoked_invite_shows_as_revoked() {
    let owner = deployed_owner();
    make_invite(owner.path(), &[]);
    let listed = stdout_of(am(owner.path()).args(["owner", "invites"]));
    let id = listed.split_whitespace().next().unwrap();
    assert!(listed.contains("valid"), "got {listed:?}");
    am(owner.path()).args(["owner", "revoke", id]).assert().success();
    am(owner.path()).args(["owner", "invites"]).assert().success().stdout(predicate::str::contains("revoked"));
    am(owner.path()).args(["owner", "revoke", "not-an-id"]).assert().code(1);
}

#[test]
fn the_printed_skill_is_complete() {
    let dir = tempfile::tempdir().unwrap();
    am(dir.path())
        .args(["skill", "--image", "registry.example/am@sha256:abc"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("---\nname: answering-machine"))
        .stdout(predicate::str::contains("registry.example/am@sha256:abc"))
        .stdout(predicate::str::contains("__").not());
}

#[test]
fn an_empty_history_says_so() {
    let dir = tempfile::tempdir().unwrap();
    am(dir.path()).args(["owner", "init", "--name", "Ada"]).assert().success();
    am(dir.path()).args(["owner", "history"]).assert().success().stdout("No saved messages.\n");
}

#[test]
fn serve_refuses_to_start_without_its_key() {
    let dir = tempfile::tempdir().unwrap();
    am(dir.path())
        .arg("serve")
        .env_remove("SERVER_SECRET_KEY_HEX")
        .env("OWNER_IDS", "00".repeat(32))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("SERVER_SECRET_KEY_HEX is not set"));
}

#[test]
fn every_error_reads_as_one_sentence_and_local_failures_exit_1() {
    // Canonical's error discipline (https://canonical.github.io/rust-best-practices/error-and-panic-discipline.html):
    // a consistent shape, starting with a verb, without repeating itself up the stack. Ours: "cannot <attempt>: <why>".
    let owner = deployed_owner();
    let empty = tempfile::tempdir().unwrap();
    let expired = make_invite(owner.path(), &["--hours", "0"]);
    let failures: [(&Path, Vec<&str>, &str); 4] = [
        (
            empty.path(),
            vec!["owner", "invite", "--label", "x"],
            "error: cannot make the invite: there is no owner identity",
        ),
        (empty.path(), vec!["leave", &expired, "--text", "hi"], "error: cannot leave the message: the invite to Ada's"),
        (
            empty.path(),
            vec!["leave", "nobody", "--text", "hi"],
            "error: cannot leave the message: there is no saved invite",
        ),
        (owner.path(), vec!["owner", "revoke", "x"], "error: cannot revoke the invite: \"x\" is not an invite id"),
    ];
    for (state, args, expected_start) in failures {
        let output = am(state).args(&args).assert().code(1).get_output().stderr.clone();
        let stderr = String::from_utf8(output).unwrap();
        assert!(stderr.starts_with(expected_start), "{args:?}: got {stderr:?}");
        assert_eq!(stderr.matches("cannot").count(), 1, "{args:?}: one 'cannot', at the front: {stderr:?}");
    }
}

#[test]
fn invite_lifetimes_are_capped_instead_of_overflowing() {
    // Plain `now + hours * 3600` with hours from the command line once overflowed (ANSSI LANG-ARITH).
    let owner = deployed_owner();
    make_invite(owner.path(), &["--hours", "720"]);
    for too_many in ["721", &u64::MAX.to_string()] {
        am(owner.path())
            .args(["owner", "invite", "--label", "x", "--hours", too_many])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("at most 720 hours"));
    }
}

#[test]
fn server_env_matches_the_manifest_for_self_hosting() {
    // Self-hosting runs the same `am serve` from an env file; it must carry exactly what the Datum manifest would.
    let dir = tempfile::tempdir().unwrap();
    am(dir.path()).args(["owner", "init", "--name", "Ada"]).assert().success();
    let env = stdout_of(am(dir.path()).args(["owner", "server-env"]));
    let manifest = stdout_of(am(dir.path()).args(["owner", "manifest", "--name", "am-test1", "--image", "img"]));
    let lines: Vec<&str> = env.lines().collect();
    assert_eq!(lines.len(), 3, "{env}");
    let key = lines[0].strip_prefix("SERVER_SECRET_KEY_HEX=").unwrap();
    assert!(manifest.contains(&format!("SERVER_SECRET_KEY_HEX: \"{key}\"")), "same server key in both");
    let owners = lines[1].strip_prefix("OWNER_IDS=").unwrap();
    assert!(manifest.contains(&format!("OWNER_IDS: \"{owners}\"")));
    assert_eq!(lines[2], "REVOKED_INVITES=");
}
