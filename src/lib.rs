//! An answering machine for a coding agent, reachable only over Iroh through Datum's relays.
//!
//! Module map:
//! - [`relays`]: Datum's relays and the endpoint every command binds.
//! - [`keys`]: secret keys as hex, in files or the environment.
//! - [`invite`]: owner-signed invites (`am1.…`), the only thing a sender needs.
//! - [`protocol`]: the two ALPNs, the request/reply shapes and the one-JSON-per-stream exchange.
//! - [`store`]: the server's SQLite mailbox.
//! - [`server`]: `am serve`, the two doors.
//! - [`owner`]: the owner's commands and their local copy of fetched messages.
//! - [`sender`]: leaving a message, and saved invites.
//! - [`state`]: the local state directory (keys, saved invites, fetched messages).
// No unsafe code, and the compiler keeps it that way (ANSSI LANG-UNSAFE, https://anssi-fr.github.io/rust-guide/).
#![forbid(unsafe_code)]

pub mod invite;
pub mod keys;
pub mod owner;
pub mod protocol;
pub mod relays;
pub mod sender;
pub mod server;
pub mod state;
pub mod store;

/// Seconds since the Unix epoch. Everything stored or signed uses this, so there is no time zone to get wrong.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `bytes` random bytes as lowercase hex, for nonces and message ids.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the OS random source works");
    data_encoding::HEXLOWER.encode(&buf)
}

/// A Unix time as `2026-10-23 18:04:11 UTC`, for people and agents reading the inbox.
pub fn format_unix(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm), to avoid a date crate for one line of output.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates_format_correctly_across_leap_years() {
        assert_eq!(super::format_unix(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(super::format_unix(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(super::format_unix(1_792_778_651), "2026-10-23 18:04:11 UTC");
    }
}
