//! Finding FreeDAWm PCs on the local network: each listening PC announces itself as a DNS-SD
//! service (`_freedawm._tcp`, its PC name as the instance name, its role in the TXT record) over
//! mDNS; a remote recorder browses for the booth by name. Pure Rust (`mdns-sd`), its own thread.

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::SERVICE_TYPE;

/// A FreeDAWm PC seen on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    /// `host:port`, IPv4 first.
    pub addr: String,
    pub role: String,
}

/// The mDNS daemon: announcing this PC and/or browsing for others. Dropped: both stop.
pub struct Discovery {
    daemon: ServiceDaemon,
    announced: Option<String>,
    found: Arc<Mutex<Vec<Found>>>,
    browsing: bool,
}

impl Discovery {
    pub fn new() -> Result<Discovery, String> {
        let daemon = ServiceDaemon::new().map_err(|e| format!("mDNS: {e}"))?;
        Ok(Discovery { daemon, announced: None, found: Arc::new(Mutex::new(Vec::new())), browsing: false })
    }

    /// Announces this PC as `name` on `port` (role `booth` or `recorder`), replacing an earlier one.
    pub fn announce(&mut self, name: &str, port: u16, role: &str) -> Result<(), String> {
        self.withdraw();
        let host = format!("{}.local.", crate::hub::computer_name().replace([' ', '.'], "-"));
        let props = [("role", role), ("v", "1")];
        let info = ServiceInfo::new(SERVICE_TYPE, name, &host, "", port, &props[..]).map_err(|e| format!("mDNS: {e}"))?.enable_addr_auto();
        let full = info.get_fullname().to_string();
        self.daemon.register(info).map_err(|e| format!("mDNS: {e}"))?;
        self.announced = Some(full);
        Ok(())
    }

    pub fn withdraw(&mut self) {
        if let Some(full) = self.announced.take() {
            let _ = self.daemon.unregister(&full);
        }
    }

    /// Starts looking for other PCs (once); [`Discovery::found`] lists them as they answer.
    pub fn browse(&mut self) -> Result<(), String> {
        if self.browsing {
            return Ok(());
        }
        let rx = self.daemon.browse(SERVICE_TYPE).map_err(|e| format!("mDNS: {e}"))?;
        self.browsing = true;
        let found = Arc::clone(&self.found);
        std::thread::Builder::new()
            .name("freedawm-mdns-browse".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv_timeout(Duration::from_secs(3600)) {
                    let mut f = found.lock().unwrap_or_else(PoisonError::into_inner);
                    match ev {
                        ServiceEvent::ServiceResolved(info) => {
                            let name = info.fullname.strip_suffix(&format!(".{SERVICE_TYPE}")).unwrap_or(&info.fullname).replace("\\", "");
                            let mut v4: Vec<_> = info.get_addresses_v4().into_iter().collect();
                            v4.sort();
                            let Some(ip) = v4.first() else { continue };
                            let role = info.get_property_val_str("role").unwrap_or("").to_string();
                            let entry = Found { name, addr: format!("{ip}:{}", info.port), role };
                            f.retain(|x| x.name != entry.name);
                            f.push(entry);
                        }
                        ServiceEvent::ServiceRemoved(_, full) => {
                            let name = full.strip_suffix(&format!(".{SERVICE_TYPE}")).unwrap_or(&full).replace("\\", "");
                            f.retain(|x| x.name != name);
                        }
                        _ => {}
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn found(&self) -> Vec<Found> {
        self.found.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// A shared view of what browsing finds (for the follower thread's name lookups).
    pub fn found_handle(&self) -> Arc<Mutex<Vec<Found>>> {
        Arc::clone(&self.found)
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.withdraw();
        let _ = self.daemon.shutdown();
    }
}
