//! JACK on Linux, and PipeWire through it (pipewire-jack answers JACK clients).
//!
//! cpal's JACK host makes the streams, but it shows one input and one output device of its own
//! and connects their ports only to `system:capture_N` / `system:playback_N`, which PipeWire does
//! not have unless configured to. So FreeDAWm lists the devices itself: each JACK client with
//! physical ports is a device (a sound card, a USB interface; under PipeWire every source and
//! sink), its capture ports an input device, its playback ports an output device. The stream
//! opens with as many channels as the chosen device has ports, and once it runs, its ports are
//! connected one to one to the device's ports (whatever cpal connected is removed first).
//!
//! libjack is loaded at run time: without it, or without a server, JACK is not listed and a
//! saved JACK choice falls back to the default audio system. No server is ever started.

// The helpers are Linux's; elsewhere only the tests use them.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::BTreeSet;

/// The audio system's name (cpal's JACK host).
pub const SYSTEM: &str = "JACK";

/// JACK's audio port type.
const AUDIO: &str = "32 bit float mono audio";

/// Whether `system` names JACK.
pub(crate) fn is_jack(system: &str) -> bool {
    system.eq_ignore_ascii_case(SYSTEM)
}

/// A device as FreeDAWm lists it: one JACK client's physical ports of one direction, in JACK's
/// order (full names, `client:port`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Group {
    pub name: String,
    pub ports: Vec<String>,
}

/// The client part of a full port name (`client:port`); client names may not contain `:`.
fn client_of(port: &str) -> &str {
    port.split_once(':').map_or(port, |(c, _)| c)
}

/// `ports` grouped by client, in the order the clients first appear.
pub(crate) fn group(ports: Vec<String>) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for port in ports {
        let client = client_of(&port);
        match groups.iter_mut().find(|g| g.name == client) {
            Some(g) => g.ports.push(port),
            None => groups.push(Group { name: client.to_string(), ports: vec![port] }),
        }
    }
    groups
}

/// The group named `name`, else the first one (the default device).
pub(crate) fn pick<'a>(groups: &'a [Group], name: Option<&str>) -> Option<&'a Group> {
    name.and_then(|n| groups.iter().find(|g| g.name == n)).or_else(|| groups.first())
}

/// A cpal stream's ports in channel order: `in_0`, `in_1`, … `in_10` (numerically, not as text).
pub(crate) fn in_channel_order(mut ports: Vec<String>) -> Vec<String> {
    ports.sort_by_key(|p| p.rsplit_once('_').and_then(|(_, n)| n.parse::<u32>().ok()).unwrap_or(u32::MAX));
    ports
}

/// What a running JACK server offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Server {
    pub rate: u32,
    pub buffer: u32,
    /// Devices to record from (physical capture ports).
    pub sources: Vec<Group>,
    /// Devices to play to (physical playback ports).
    pub sinks: Vec<Group>,
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{AUDIO, BTreeSet, Group, Server, client_of, group, in_channel_order};
    use jack::{Client, ClientOptions, PortFlags};

    /// A short-lived client of our own, never starting a server; `None` without libjack or a
    /// running server.
    fn client(name: &str) -> Option<Client> {
        match Client::new(name, ClientOptions::NO_START_SERVER) {
            Ok((c, _)) => Some(c),
            Err(e) => {
                log::debug!("JACK: no server ({e})");
                None
            }
        }
    }

    fn physical(c: &Client, flags: PortFlags) -> Vec<Group> {
        group(c.ports(None, Some(AUDIO), flags | PortFlags::IS_PHYSICAL))
    }

    pub(crate) fn probe() -> Option<Server> {
        let c = client("FreeDAWm probe")?;
        Some(Server {
            rate: c.sample_rate(),
            buffer: c.buffer_size(),
            // A capture port is a source of audio: an output in JACK's terms.
            sources: physical(&c, PortFlags::IS_OUTPUT),
            sinks: physical(&c, PortFlags::IS_INPUT),
        })
    }

    /// The clients whose names start with `prefix` (a cpal stream's client is named after its
    /// device; JACK appends `-01`, … when the name is taken).
    pub(crate) fn clients_named(prefix: &str) -> BTreeSet<String> {
        let Some(c) = client("FreeDAWm ports") else { return BTreeSet::new() };
        c.ports(None, Some(AUDIO), PortFlags::empty()).iter().map(|p| client_of(p)).filter(|n| n.starts_with(prefix)).map(str::to_string).collect()
    }

    pub(crate) fn connect(prefix: &str, before: &BTreeSet<String>, device: &str, output: bool) -> Result<usize, String> {
        let c = client("FreeDAWm ports").ok_or("the JACK server went away")?;
        // The stream's own ports: a client named after the cpal device that was not there before.
        let (ours_flag, theirs_flag) = if output { (PortFlags::IS_OUTPUT, PortFlags::IS_INPUT) } else { (PortFlags::IS_INPUT, PortFlags::IS_OUTPUT) };
        let all = c.ports(None, Some(AUDIO), ours_flag);
        let new: Vec<String> =
            all.iter().map(|p| client_of(p)).filter(|n| n.starts_with(prefix) && !before.contains(*n)).map(str::to_string).collect();
        let mine = match new.first() {
            Some(m) if new.iter().all(|n| n == m) => m.clone(),
            Some(_) => return Err("more than one new FreeDAWm client; not connecting".into()),
            None => return Err("the stream's ports are not there".into()),
        };
        let ours = in_channel_order(all.into_iter().filter(|p| client_of(p) == mine).collect());
        let theirs = group(c.ports(None, Some(AUDIO), theirs_flag | PortFlags::IS_PHYSICAL))
            .into_iter()
            .find(|g| g.name == device)
            .map(|g| g.ports)
            .ok_or_else(|| format!("no JACK device “{device}”"))?;
        let mut made = 0;
        for (i, port) in ours.iter().enumerate() {
            // Whatever cpal (or anyone) connected: the chosen device only.
            if let Some(p) = c.port_by_name(port) {
                for other in p.get_connections() {
                    let (src, dst) = if output { (port.as_str(), other.as_str()) } else { (other.as_str(), port.as_str()) };
                    if let Err(e) = c.disconnect_ports_by_name(src, dst) {
                        log::warn!("JACK: cannot disconnect {src} → {dst}: {e}");
                    }
                }
            }
            let Some(target) = theirs.get(i) else { continue };
            let (src, dst) = if output { (port.as_str(), target.as_str()) } else { (target.as_str(), port.as_str()) };
            match c.connect_ports_by_name(src, dst) {
                Ok(()) => made += 1,
                Err(e) => log::warn!("JACK: cannot connect {src} → {dst}: {e}"),
            }
        }
        Ok(made)
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{BTreeSet, Server};

    pub(crate) fn probe() -> Option<Server> {
        None
    }

    pub(crate) fn clients_named(_prefix: &str) -> BTreeSet<String> {
        BTreeSet::new()
    }

    pub(crate) fn connect(_prefix: &str, _before: &BTreeSet<String>, _device: &str, _output: bool) -> Result<usize, String> {
        Err("JACK is not supported on this platform".into())
    }
}

/// The running server and its devices; `None` without libjack or a server.
pub(crate) fn probe() -> Option<Server> {
    imp::probe()
}

/// A stream about to be opened on JACK: the device to connect it to once it runs.
#[derive(Debug)]
pub(crate) struct Route {
    /// The cpal device's (client's) name.
    prefix: String,
    /// The clients so named before the stream opened.
    before: BTreeSet<String>,
    /// The chosen device.
    pub group: Group,
    output: bool,
}

impl Route {
    /// For a stream on the cpal JACK device named `cpal_name` to the device named `want` (else
    /// the first); `None` when the server lists no such devices.
    pub(crate) fn prepare(cpal_name: &str, want: Option<&str>, output: bool) -> Option<Route> {
        let server = probe()?;
        let groups = if output { &server.sinks } else { &server.sources };
        let group = pick(groups, want)?.clone();
        Some(Route { prefix: cpal_name.to_string(), before: imp::clients_named(cpal_name), group, output })
    }

    /// The device's channel names: its ports' own names (`playback_FL`).
    pub(crate) fn channel_names(&self) -> Vec<String> {
        self.group.ports.iter().map(|p| p.split_once(':').map_or(p.as_str(), |(_, n)| n).to_string()).collect()
    }

    /// The stream's channel count: one per port of the device.
    pub(crate) fn channels(&self) -> u16 {
        u16::try_from(self.group.ports.len()).unwrap_or(u16::MAX).clamp(1, 64)
    }

    /// Connects the running stream's ports to the device's (logged; a failure leaves the stream
    /// running, connectable by hand in a patchbay).
    pub(crate) fn connect(&self) {
        match imp::connect(&self.prefix, &self.before, &self.group.name, self.output) {
            Ok(n) => log::info!("JACK: {n} port(s) connected to “{}”", self.group.name),
            Err(e) => log::warn!("JACK: not connected to “{}”: {e}", self.group.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Group, group, in_channel_order, is_jack, pick};

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ports_group_by_client_in_order() {
        let g = group(names(&[
            "Built-in Audio Analog Stereo:capture_FL",
            "Scarlett 2i2 USB:capture_AUX0",
            "Built-in Audio Analog Stereo:capture_FR",
            "Scarlett 2i2 USB:capture_AUX1",
        ]));
        assert_eq!(
            g,
            vec![
                Group {
                    name: "Built-in Audio Analog Stereo".into(),
                    ports: names(&["Built-in Audio Analog Stereo:capture_FL", "Built-in Audio Analog Stereo:capture_FR"])
                },
                Group { name: "Scarlett 2i2 USB".into(), ports: names(&["Scarlett 2i2 USB:capture_AUX0", "Scarlett 2i2 USB:capture_AUX1"]) },
            ]
        );
        assert_eq!(pick(&g, Some("Scarlett 2i2 USB")).map(|g| g.name.as_str()), Some("Scarlett 2i2 USB"));
        // An unplugged device: the default (first) one.
        assert_eq!(pick(&g, Some("Gone")).map(|g| g.name.as_str()), Some("Built-in Audio Analog Stereo"));
        assert_eq!(pick(&g, None).map(|g| g.name.as_str()), Some("Built-in Audio Analog Stereo"));
        assert_eq!(pick(&[], None), None);
    }

    #[test]
    fn stream_ports_sort_by_channel_number() {
        let p = in_channel_order(names(&["c:in_10", "c:in_2", "c:in_0", "c:in_1"]));
        assert_eq!(p, names(&["c:in_0", "c:in_1", "c:in_2", "c:in_10"]));
        assert!(is_jack("jack") && is_jack("JACK") && !is_jack("ALSA"));
    }
}
