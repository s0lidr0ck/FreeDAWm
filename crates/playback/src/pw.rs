//! PipeWire's "All sources": one input device made of any PipeWire source channels.
//!
//! On PipeWire every microphone, interface input, virtual device (Weir's, a loopback), sink
//! monitor and application stream is a node with its own output ports. FreeDAWm's input on the
//! "All sources" device is one capture stream that PipeWire does not connect by itself: channel
//! `k` records the source port `AudioConfig::input_sources[k]` (kept per machine by `node:port`
//! name, which survives restarts where ids do not). Once the stream runs, [`connect`] links each
//! of its ports to its source. An empty list means the system's default source.
//!
//! cpal's PipeWire host makes the stream; this module only looks at the graph and makes links.
//! cpal shuts PipeWire down when its last host goes (and PipeWire cannot start again in that
//! process), so every look here holds a cpal PipeWire host while it uses PipeWire objects.

// The graph helpers are Linux's; elsewhere only the tests use them.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// The input device that records any sources (I/O Setup picks them).
pub const ALL_SOURCES: &str = "All sources";

/// Whether `system` names PipeWire.
pub(crate) fn is_pipewire(system: &str) -> bool {
    system.eq_ignore_ascii_case("PipeWire")
}

/// A node as the graph shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Node {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub class: String,
    /// The process that made it (application streams and our own), when the node says.
    pub pid: Option<u32>,
    /// The client that made it.
    pub client: Option<u32>,
}

/// A port as the graph shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Port {
    pub id: u32,
    pub node: u32,
    pub name: String,
    /// An output port (audio comes out of it).
    pub output: bool,
    pub audio: bool,
    /// The channel (`FL`, `MONO`, `AUX3`), when the port says.
    pub channel: Option<String>,
    /// The port's index on its node, for channel order.
    pub index: u32,
}

/// The graph at one moment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Graph {
    pub nodes: Vec<Node>,
    pub ports: Vec<Port>,
    /// `node.name` of the system's default source.
    pub default_source: Option<String>,
    /// Links, as (output port, input port) ids.
    pub links: Vec<(u32, u32)>,
    /// Clients' processes, as (client id, process id).
    pub clients: Vec<(u32, u32)>,
}

/// One source channel to record from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `node.name:port.name`, as `AudioConfig::input_sources` keeps it.
    pub key: String,
    /// What the user reads: "Stream Mic (Weir) · FL", "Monitor of Speakers · FR".
    pub label: String,
    /// The node's own label, to group a node's channels (a stereo pair).
    pub node_label: String,
}

/// Which kind of node a source comes from, in the order they are listed: devices (and virtual
/// devices) first, then monitors of outputs, then application streams.
fn rank(class: &str) -> u8 {
    if class.starts_with("Audio/Source") || class.starts_with("Audio/Duplex") {
        0
    } else if class.starts_with("Audio/Sink") {
        1
    } else {
        2
    }
}

impl Graph {
    fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// The process that made `n`: its own say, else its client's.
    fn pid(&self, n: &Node) -> Option<u32> {
        n.pid.or_else(|| n.client.and_then(|c| self.clients.iter().find(|(id, _)| *id == c)).map(|(_, pid)| *pid))
    }

    fn ours(&self, n: &Node, own_pid: u32) -> bool {
        self.pid(n) == Some(own_pid)
    }

    /// Every audio output port of another process's nodes, as sources: devices first, then
    /// monitors, then application streams, each node's channels in order.
    pub(crate) fn sources(&self, own_pid: u32) -> Vec<Source> {
        let mut nodes: Vec<&Node> = self.nodes.iter().filter(|n| !self.ours(n, own_pid) && !n.class.is_empty()).collect();
        nodes.sort_by_key(|n| (rank(&n.class), n.id));
        let mut out = Vec::new();
        for n in nodes {
            let mut ports: Vec<&Port> = self.ports.iter().filter(|p| p.node == n.id && p.output && p.audio).collect();
            ports.sort_by_key(|p| p.index);
            let label = if n.description.is_empty() { n.name.clone() } else { n.description.clone() };
            for p in ports {
                let channel = p.channel.clone().unwrap_or_else(|| p.name.clone());
                let node_label = if p.name.starts_with("monitor_") { format!("Monitor of {label}") } else { label.clone() };
                out.push(Source { key: format!("{}:{}", n.name, p.name), label: format!("{node_label} · {channel}"), node_label });
            }
        }
        out
    }

    /// The default source's channels (else the first device source's): what an empty source
    /// list records.
    pub(crate) fn default_sources(&self, own_pid: u32) -> Vec<String> {
        let all = self.sources(own_pid);
        let default = self.default_source.as_deref().and_then(|d| self.nodes.iter().find(|n| n.name == d));
        let node = default.or_else(|| self.nodes.iter().filter(|n| rank(&n.class) == 0 && !self.ours(n, own_pid)).min_by_key(|n| n.id));
        match node {
            Some(n) => all.into_iter().filter(|s| s.key.split_once(':').is_some_and(|(name, _)| name == n.name)).map(|s| s.key).collect(),
            None => Vec::new(),
        }
    }

    /// The port `key` (`node.name:port.name`) names, with its node.
    fn port_by_key(&self, key: &str) -> Option<&Port> {
        let (node, port) = key.split_once(':')?;
        self.ports.iter().find(|p| p.name == port && p.output && self.node(p.node).is_some_and(|n| n.name == node))
    }

    /// The sources (`node:port`) linked to each of our capture stream's channels, in order.
    #[cfg(test)]
    pub(crate) fn our_sources(&self, own_pid: u32) -> Vec<Vec<String>> {
        let Some((_, ours)) = self.our_inputs(own_pid) else { return Vec::new() };
        ours.iter()
            .map(|to| {
                self.links
                    .iter()
                    .filter(|(_, i)| *i == to.id)
                    .filter_map(|(o, _)| self.ports.iter().find(|p| p.id == *o))
                    .filter_map(|p| self.node(p.node).map(|n| format!("{}:{}", n.name, p.name)))
                    .collect()
            })
            .collect()
    }

    /// Our capture stream (the newest of this process) and its input ports in channel order.
    fn our_inputs(&self, own_pid: u32) -> Option<(u32, Vec<&Port>)> {
        let node = self.nodes.iter().filter(|n| self.ours(n, own_pid) && n.class.starts_with("Stream/Input/Audio")).max_by_key(|n| n.id)?;
        let mut ports: Vec<&Port> = self.ports.iter().filter(|p| p.node == node.id && !p.output && p.audio).collect();
        ports.sort_by_key(|p| p.index);
        Some((node.id, ports))
    }
}

/// The labels of `keys` (sources not in the graph now read "… (not available)").
pub fn labels(keys: &[String], sources: &[Source]) -> Vec<String> {
    keys.iter().map(|k| sources.iter().find(|s| &s.key == k).map_or_else(|| format!("{k} (not available)"), |s| s.label.clone())).collect()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Graph, Node, Port};
    use pipewire as pw;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn prop(props: &pw::spa::utils::dict::DictRef, key: &str) -> Option<String> {
        props.get(key).map(str::to_string)
    }

    /// The name in a `default.audio.source` value (`{"name":"alsa_input…"}`).
    pub(crate) fn default_name(value: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(value).ok()?;
        v.get("name")?.as_str().map(str::to_string)
    }

    /// A connection to the PipeWire server, its registry read into a [`Graph`].
    pub(crate) struct Session {
        // Declared first so it drops last: keeps PipeWire initialised (see the module docs).
        _cpal: cpal::platform::PipeWireHost,
        mainloop: pw::main_loop::MainLoopRc,
        core: pw::core::CoreRc,
        _context: pw::context::ContextRc,
    }

    impl Session {
        pub(crate) fn open() -> Option<Session> {
            let host = cpal::platform::PipeWireHost::new().ok()?;
            pw::init();
            let mainloop = pw::main_loop::MainLoopRc::new(None).ok()?;
            let context = pw::context::ContextRc::new(&mainloop, None).ok()?;
            let core = context.connect_rc(None).ok()?;
            Some(Session { _cpal: host, mainloop, core, _context: context })
        }

        /// Processes events until the server has answered everything sent so far.
        fn roundtrip(&self) {
            let done = Rc::new(Cell::new(false));
            let Ok(pending) = self.core.sync(0) else { return };
            let (d, l) = (Rc::clone(&done), self.mainloop.clone());
            let _listener = self
                .core
                .add_listener_local()
                .done(move |id, seq| {
                    if id == pw::core::PW_ID_CORE && seq == pending {
                        d.set(true);
                        l.quit();
                    }
                })
                .register();
            while !done.get() {
                self.mainloop.run();
            }
        }

        /// The nodes and ports now, and the default source.
        pub(crate) fn graph(&self) -> Graph {
            let Ok(registry) = self.core.get_registry_rc() else { return Graph::default() };
            let graph = Rc::new(RefCell::new(Graph::default()));
            let metas: Rc<RefCell<Vec<pw::metadata::Metadata>>> = Rc::new(RefCell::new(Vec::new()));
            let (g, m, r) = (Rc::clone(&graph), Rc::clone(&metas), registry.clone());
            let _listener = registry
                .add_listener_local()
                .global(move |global| {
                    let Some(props) = global.props.as_ref() else { return };
                    match global.type_ {
                        pw::types::ObjectType::Node => {
                            let client = prop(props, "client.id").and_then(|c| c.parse::<u32>().ok());
                            g.borrow_mut().nodes.push(Node {
                                id: global.id,
                                name: prop(props, "node.name").unwrap_or_default(),
                                description: prop(props, "node.description").or_else(|| prop(props, "node.nick")).unwrap_or_default(),
                                class: prop(props, "media.class").unwrap_or_default(),
                                pid: prop(props, "application.process.id").and_then(|p| p.parse().ok()),
                                client,
                            });
                        }
                        // A node's process is its client's.
                        pw::types::ObjectType::Client => {
                            let pid = prop(props, "application.process.id").or_else(|| prop(props, "pipewire.sec.pid")).and_then(|p| p.parse().ok());
                            if let Some(pid) = pid {
                                g.borrow_mut().clients.push((global.id, pid));
                            }
                        }
                        pw::types::ObjectType::Port => g.borrow_mut().ports.push(Port {
                            id: global.id,
                            node: prop(props, "node.id").and_then(|n| n.parse().ok()).unwrap_or(u32::MAX),
                            name: prop(props, "port.name").unwrap_or_default(),
                            output: prop(props, "port.direction").as_deref() == Some("out"),
                            audio: prop(props, "format.dsp").is_some_and(|f| f.contains("audio")),
                            channel: prop(props, "audio.channel"),
                            index: prop(props, "port.id").and_then(|n| n.parse().ok()).unwrap_or(u32::MAX),
                        }),
                        pw::types::ObjectType::Link => {
                            let id = |k: &str| prop(props, k).and_then(|n| n.parse().ok());
                            if let (Some(from), Some(to)) = (id("link.output.port"), id("link.input.port")) {
                                g.borrow_mut().links.push((from, to));
                            }
                        }
                        pw::types::ObjectType::Metadata if prop(props, "metadata.name").as_deref() == Some("default") => {
                            if let Ok(meta) = r.bind::<pw::metadata::Metadata, _>(global) {
                                m.borrow_mut().push(meta);
                            }
                        }
                        _ => {}
                    }
                })
                .register();
            self.roundtrip();
            // The default metadata's properties arrive once it is bound and listened to.
            let defaults: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
            let listeners: Vec<pw::metadata::MetadataListener> = metas
                .borrow()
                .iter()
                .map(|meta| {
                    let d = Rc::clone(&defaults);
                    meta.add_listener_local()
                        .property(move |_, key, _, value| {
                            if key == Some("default.audio.source")
                                && let Some(name) = value.and_then(default_name)
                            {
                                *d.borrow_mut() = Some(name);
                            }
                            0
                        })
                        .register()
                })
                .collect();
            self.roundtrip();
            drop(listeners);
            let mut out = graph.borrow().clone();
            out.default_source = defaults.borrow().clone();
            out
        }

        /// Links `from` (an output port) to `to` (an input port); the link stays after this
        /// session ends (keep the proxy until [`Session::flush`]) and goes when either port does.
        pub(crate) fn link(&self, from: &Port, to: &Port) -> Result<pw::link::Link, String> {
            let props = pw::properties::properties! {
                "link.output.node" => from.node.to_string(),
                "link.output.port" => from.id.to_string(),
                "link.input.node" => to.node.to_string(),
                "link.input.port" => to.id.to_string(),
                "object.linger" => "true",
            };
            self.core.create_object::<pw::link::Link>("link-factory", &props).map_err(|e| e.to_string())
        }

        pub(crate) fn flush(&self) {
            self.roundtrip();
        }
    }
}

/// The graph now; `None` without a PipeWire server.
#[cfg(target_os = "linux")]
pub(crate) fn graph() -> Option<Graph> {
    imp::Session::open().map(|s| s.graph())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn graph() -> Option<Graph> {
    None
}

/// Every source FreeDAWm can record on "All sources" now (empty without PipeWire).
pub fn sources() -> Vec<Source> {
    graph().map(|g| g.sources(std::process::id())).unwrap_or_default()
}

/// The source list a configuration records: its own, else the default source's channels.
pub fn effective_sources(chosen: &[String]) -> Vec<String> {
    if !chosen.is_empty() {
        return chosen.to_vec();
    }
    graph().map(|g| g.default_sources(std::process::id())).unwrap_or_default()
}

/// Links this process's capture stream (the newest) to `keys`, channel by channel, waiting up
/// to a second for its ports to appear. The number of links made.
#[cfg(target_os = "linux")]
pub(crate) fn connect(keys: &[String]) -> Result<usize, String> {
    let session = imp::Session::open().ok_or("PipeWire is not running")?;
    let pid = std::process::id();
    let mut graph = session.graph();
    for _ in 0..20 {
        if graph.our_inputs(pid).is_some_and(|(_, p)| !p.is_empty() && p.len() >= keys.len()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        graph = session.graph();
    }
    let (_, ours) = graph.our_inputs(pid).ok_or("the input stream is not in the PipeWire graph")?;
    let mut links = Vec::new();
    for (key, to) in keys.iter().zip(ours) {
        match graph.port_by_key(key) {
            Some(from) => match session.link(from, to) {
                Ok(l) => links.push(l),
                Err(e) => log::warn!("PipeWire: cannot link {key}: {e}"),
            },
            None => log::warn!("PipeWire: source {key} is not available"),
        }
    }
    session.flush();
    Ok(links.len())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn connect(_keys: &[String]) -> Result<usize, String> {
    Err("PipeWire is not supported on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::{Graph, Node, Port, labels};

    fn node(id: u32, name: &str, description: &str, class: &str, pid: Option<u32>) -> Node {
        Node { id, name: name.into(), description: description.into(), class: class.into(), pid, client: None }
    }

    fn port(id: u32, node: u32, name: &str, output: bool, index: u32) -> Port {
        let channel = name.rsplit_once('_').map(|(_, c)| c.to_string());
        Port { id, node, name: name.into(), output, audio: true, channel, index }
    }

    fn graph() -> Graph {
        Graph {
            nodes: vec![
                node(40, "firefox", "Firefox", "Stream/Output/Audio", Some(77)),
                node(31, "mic-a", "Mic A", "Audio/Source/Virtual", None),
                node(33, "speakers", "Speakers", "Audio/Sink", None),
                node(32, "weir-stream", "Stream Mic (Weir)", "Audio/Source/Virtual", None),
                // Ours, known by its client's process (as the registry tells it).
                Node { client: Some(9), ..node(50, "freedawm", "", "Stream/Input/Audio", None) },
                node(51, "freedawm-out", "", "Stream/Output/Audio", Some(1)),
            ],
            ports: vec![
                port(100, 32, "capture_FR", true, 1),
                port(101, 32, "capture_FL", true, 0),
                port(102, 31, "capture_MONO", true, 0),
                port(103, 33, "monitor_FL", true, 0),
                port(104, 33, "playback_FL", false, 1),
                port(105, 40, "output_FL", true, 0),
                port(106, 50, "input_AUX1", false, 1),
                port(107, 50, "input_AUX0", false, 0),
                port(108, 51, "output_FL", true, 0),
                Port { audio: false, ..port(109, 31, "midi_out", true, 1) },
            ],
            default_source: Some("weir-stream".into()),
            links: vec![(100, 107), (102, 106), (101, 106)],
            clients: vec![(9, 1)],
        }
    }

    #[test]
    fn sources_list_devices_then_monitors_then_apps_never_our_own() {
        let s = graph().sources(1);
        let keys: Vec<&str> = s.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["mic-a:capture_MONO", "weir-stream:capture_FL", "weir-stream:capture_FR", "speakers:monitor_FL", "firefox:output_FL"]);
        let labels: Vec<&str> = s.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Mic A · MONO", "Stream Mic (Weir) · FL", "Stream Mic (Weir) · FR", "Monitor of Speakers · FL", "Firefox · FL"]);
    }

    #[test]
    fn the_default_source_fills_an_empty_list() {
        assert_eq!(graph().default_sources(1), ["weir-stream:capture_FL", "weir-stream:capture_FR"]);
        // No default known: the first device source.
        let g = Graph { default_source: None, ..graph() };
        assert_eq!(g.default_sources(1), ["mic-a:capture_MONO"]);
    }

    #[test]
    fn our_inputs_are_found_in_channel_order_and_sources_by_key() {
        let g = graph();
        let (node, ports) = g.our_inputs(1).unwrap();
        assert_eq!(node, 50);
        assert_eq!(ports.iter().map(|p| p.id).collect::<Vec<_>>(), [107, 106]);
        assert_eq!(g.port_by_key("weir-stream:capture_FR").map(|p| p.id), Some(100));
        assert_eq!(g.port_by_key("speakers:playback_FL"), None, "an input port is no source");
        assert_eq!(g.our_sources(1), [vec!["weir-stream:capture_FR"], vec!["mic-a:capture_MONO", "weir-stream:capture_FL"]]);
        let keys = vec!["mic-a:capture_MONO".to_string(), "gone:capture_FL".to_string()];
        assert_eq!(labels(&keys, &g.sources(1)), ["Mic A · MONO", "gone:capture_FL (not available)"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_source_names_parse() {
        assert_eq!(super::imp::default_name(r#"{"name":"alsa_input.usb-Arctis"}"#).as_deref(), Some("alsa_input.usb-Arctis"));
        assert_eq!(super::imp::default_name("garbage"), None);
    }
}
