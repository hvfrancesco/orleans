//! Local-network discovery of FluidNC / Grbl_ESP32 / ESP3D controllers.
//!
//! The controllers expose their web interface through mDNS. Browse HTTP
//! services and use the resolved IPv4 address for the raw Telnet connection
//! on port 23.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use mdns_sd::{RecvTimeoutError, ServiceDaemon, ServiceEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredMachine {
    pub name: String,
    pub address: String,
    pub hostname: String,
    pub http_port: u16,
}

pub fn discover(timeout: Duration) -> Result<Vec<DiscoveredMachine>, String> {
    let daemon = ServiceDaemon::new().map_err(|err| err.to_string())?;
    let receiver = daemon
        .browse("_http._tcp.local.")
        .map_err(|err| err.to_string())?;
    let deadline = Instant::now() + timeout;
    let mut found: BTreeMap<String, DiscoveredMachine> = BTreeMap::new();

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let wait = (deadline - now).min(Duration::from_millis(200));
        match receiver.recv_timeout(wait) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let hostname = info
                    .get_hostname()
                    .trim_end_matches('.')
                    .to_ascii_lowercase();
                let mut addresses: Vec<_> = info.get_addresses_v4().into_iter().collect();
                addresses.sort_unstable();
                let address = addresses
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| hostname.clone());
                if address.is_empty() {
                    continue;
                }
                let fullname = info.get_fullname();
                let name = fullname
                    .split("._http._tcp.local.")
                    .next()
                    .unwrap_or(fullname)
                    .trim_end_matches('.')
                    .to_owned();
                let key = format!("{address}:{}", info.get_port());
                found.insert(
                    key,
                    DiscoveredMachine {
                        name: if name.is_empty() {
                            hostname.clone()
                        } else {
                            name
                        },
                        address,
                        hostname,
                        http_port: info.get_port(),
                    },
                );
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}
