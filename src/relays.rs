//! Datum's Iroh relays, and the endpoint every `am` command binds.
use anyhow::{Result, anyhow};
use iroh::{
    Endpoint, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr,
    address_lookup::{AddressLookup, EndpointData, EndpointInfo, Error, Item, memory::MemoryLookup},
    endpoint::presets,
};
use n0_future::{StreamExt, boxed::BoxStream, stream};

/// Datum's relays, used unless `RELAYS` (comma-separated URLs) says otherwise.
const DATUM_RELAYS: &str = "https://iroh-relay.us-west-1.datumconnect.net,\
https://iroh-relay.us-central-1.datumconnect.net,\
https://iroh-relay.us-east-1.datumconnect.net";

fn relay_urls() -> Result<Vec<RelayUrl>> {
    let configured = std::env::var("RELAYS").unwrap_or_else(|_| DATUM_RELAYS.to_string());
    configured
        .split(',')
        .map(|url| url.trim().parse().map_err(|e| anyhow!("the relay URL {url:?} is invalid: {e}")))
        .collect()
}

/// There is no discovery service: every endpoint here is homed on one of the relays, so "try all of them" is a complete
/// answer for any id. This keeps n0's DNS and pkarr servers out of the picture.
#[derive(Debug)]
struct DatumRelayLookup(Vec<RelayUrl>);

impl AddressLookup for DatumRelayLookup {
    fn resolve(&self, endpoint_id: EndpointId) -> Option<BoxStream<Result<Item, Error>>> {
        let data = EndpointData::new(self.0.iter().cloned().map(TransportAddr::Relay).collect());
        let item = Item::new(EndpointInfo::from_parts(endpoint_id, data), "datum-relays", None);
        Some(stream::iter([Ok(item)]).boxed())
    }
}

/// An endpoint on Datum's relays that accepts `alpns` (empty for a client).
pub async fn bind(key: SecretKey, alpns: Vec<Vec<u8>>) -> Result<Endpoint> {
    let relays = relay_urls()?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .relay_mode(RelayMode::custom(relays.clone()))
        .address_lookup(DatumRelayLookup(relays))
        .alpns(alpns)
        .bind()
        .await?;
    Ok(endpoint)
}

/// An endpoint with no relays, for tests: it finds peers only through `lookup`, which the test fills with their loopback
/// addresses. Everything else about it is the same as [`bind`], so tests run the real code paths.
pub async fn bind_local(key: SecretKey, alpns: Vec<Vec<u8>>, lookup: MemoryLookup) -> Result<Endpoint> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .relay_mode(RelayMode::Disabled)
        .address_lookup(lookup)
        .alpns(alpns)
        .bind()
        .await?;
    Ok(endpoint)
}
