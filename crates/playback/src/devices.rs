//! Audio device selection: which audio system (cpal host), output and input device, sample rate
//! and buffer size to open. The choice is per machine (the app keeps it with its preferences, not
//! in sessions). Anything that cannot be honoured falls back to the system default: a device that
//! is unplugged, a rate or buffer size the device does not offer.

use cpal::traits::{DeviceTrait, HostTrait};

/// Rates offered when a device supports them (any rate inside a supported range is possible, but
/// these are the ones interfaces actually run at).
pub const STANDARD_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Buffer sizes offered when the device reports a range that contains them.
pub const STANDARD_BUFFERS: [u32; 9] = [32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// Sample formats the streams convert to and from, best first (ASIO drivers usually run in
/// 32-bit integers, WASAPI and Core Audio in float).
pub(crate) const FORMATS: [cpal::SampleFormat; 3] = [cpal::SampleFormat::F32, cpal::SampleFormat::I32, cpal::SampleFormat::I16];

/// Preference of a sample format (lower is better); `None` when it is not supported.
pub(crate) fn format_rank(f: cpal::SampleFormat) -> Option<usize> {
    FORMATS.iter().position(|x| *x == f)
}

/// The user's audio device choice. Every field is optional: `None` means "the system default".
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Audio system (cpal host name: "WASAPI", "ASIO", "CoreAudio", "ALSA", "JACK", ...).
    pub system: Option<String>,
    pub output: Option<String>,
    pub input: Option<String>,
    /// Device sample rate; the mix resamples when it differs from the session's.
    pub sample_rate: Option<u32>,
    /// Device buffer in frames.
    pub buffer_size: Option<u32>,
    /// PipeWire's "All sources" input: the source each input channel records, in channel order
    /// (`node.name:port.name`, see `pw`). Empty: the system's default source.
    pub input_sources: Vec<String>,
}

impl AudioConfig {
    /// The audio system this configuration opens: the chosen one, else the default.
    pub fn system_name(&self) -> String {
        self.system.clone().unwrap_or_else(default_system)
    }

    /// Whether the input is PipeWire's "All sources" (the default input on PipeWire).
    pub fn all_sources(&self) -> bool {
        crate::pw::is_pipewire(&self.system_name()) && self.input.as_deref().is_none_or(|i| i == crate::pw::ALL_SOURCES)
    }
}

/// One device as the settings window lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    /// Most channels any usable configuration offers.
    pub max_channels: u16,
    /// Standard rates the device accepts (in a format the streams convert).
    pub sample_rates: Vec<u32>,
    /// Buffer range the device reports (`None` when it does not say).
    pub buffer_min: Option<u32>,
    pub buffer_max: Option<u32>,
}

impl DeviceInfo {
    /// Standard buffer sizes inside the device's range (all of them when the range is unknown).
    pub fn buffer_sizes(&self) -> Vec<u32> {
        STANDARD_BUFFERS.iter().copied().filter(|b| self.buffer_min.is_none_or(|m| *b >= m) && self.buffer_max.is_none_or(|m| *b <= m)).collect()
    }
}

/// An audio system and its devices.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AudioSystem {
    pub name: String,
    pub is_default: bool,
    pub outputs: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
}

/// Names for devices whose systems give them `(name, driver id)`: the name, with the driver id
/// after it ("HDA Intel PCH (hw:CARD=PCH,DEV=0)") when another device has the same name. ALSA
/// lists each variant of a card (`hw:`, `plughw:`, `front:`…) under the card's name.
pub(crate) fn unique_names(items: &[(String, Option<String>)]) -> Vec<String> {
    items
        .iter()
        .map(|(name, driver)| match driver {
            Some(d) if items.iter().filter(|(n, _)| n == name).count() > 1 && d != name => format!("{name} ({d})"),
            _ => name.clone(),
        })
        .collect()
}

/// `host`'s output (or input) devices under the names FreeDAWm shows and stores (see
/// [`unique_names`]).
pub(crate) fn named_devices(host: &cpal::Host, output: bool) -> Vec<(String, cpal::Device)> {
    let devices: Vec<cpal::Device> = if output { host.output_devices().ok() } else { host.input_devices().ok() }.into_iter().flatten().collect();
    let described: Vec<(cpal::Device, (String, Option<String>))> = devices
        .into_iter()
        .filter_map(|d| {
            let desc = d.description().ok()?;
            let named = (desc.name().to_string(), desc.driver().map(str::to_string));
            Some((d, named))
        })
        .collect();
    let names = unique_names(&described.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>());
    names.into_iter().zip(described.into_iter().map(|(d, _)| d)).collect()
}

/// The name FreeDAWm gives `d`, one of `host`'s output (or input) devices.
pub(crate) fn device_name(host: &cpal::Host, d: &cpal::Device, output: bool) -> Option<String> {
    named_devices(host, output).into_iter().find(|(_, x)| x == d).map(|(n, _)| n).or_else(|| d.description().ok().map(|x| x.name().to_string()))
}

fn describe(name: String, d: &cpal::Device, default: Option<&cpal::Device>, output: bool) -> Option<DeviceInfo> {
    let is_default = default == Some(d);
    let ranges: Vec<cpal::SupportedStreamConfigRange> = if output {
        d.supported_output_configs().ok()?.filter(|c| format_rank(c.sample_format()).is_some()).collect()
    } else {
        d.supported_input_configs().ok()?.filter(|c| format_rank(c.sample_format()).is_some()).collect()
    };
    let max_channels = ranges.iter().map(cpal::SupportedStreamConfigRange::channels).max().unwrap_or(0);
    let sample_rates = STANDARD_RATES.iter().copied().filter(|r| ranges.iter().any(|c| c.contains_rate(*r))).collect();
    let (mut buffer_min, mut buffer_max) = (None::<u32>, None::<u32>);
    for c in &ranges {
        if let cpal::SupportedBufferSize::Range { min, max } = c.buffer_size() {
            buffer_min = Some(buffer_min.map_or(*min, |m| m.min(*min)));
            buffer_max = Some(buffer_max.map_or(*max, |m| m.max(*max)));
        }
    }
    Some(DeviceInfo { is_default, name, max_channels, sample_rates, buffer_min, buffer_max })
}

/// The device a [`crate::Player`] has open, as [`audio_systems_with`] needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDevice {
    /// The audio system (cpal host name).
    pub system: String,
    pub output: String,
    pub input: Option<String>,
    pub rate: u32,
    pub out_channels: u16,
    pub in_channels: Option<u16>,
    pub buffer: Option<u32>,
}

/// Every audio system on this machine with its devices, when **no device is open**: each system
/// is probed (ASIO loads every driver to ask it). With a device open, call
/// [`audio_systems_with`]. Enumerating can take a moment: call it when the settings window
/// opens, not every frame.
pub fn audio_systems() -> Vec<AudioSystem> {
    audio_systems_with(None)
}

/// Every audio system with its devices, `live` being the device the player has open.
///
/// ASIO is the special case: the SDK keeps one driver per process, cpal makes a new ASIO host
/// (and a new driver slot) for every `host_from_id`, and dropping a probed driver tears it down.
/// Probing ASIO while a driver streams therefore initialised the running driver a second time and
/// then destroyed it under the audio thread (an access violation in the Playback Engine window
/// on Windows, 0.9.4). So with a device open, ASIO is listed from the registry without loading
/// anything: the running driver with its live figures, the others by name alone (their channels
/// and rates show once chosen).
pub fn audio_systems_with(live: Option<&LiveDevice>) -> Vec<AudioSystem> {
    let default_id = cpal::default_host().id();
    cpal::available_hosts()
        .into_iter()
        .filter_map(|id| {
            let is_default = id == default_id;
            if id.name().eq_ignore_ascii_case("ASIO") && live.is_some() {
                return Some(asio_system(asio_driver_names(), live, is_default));
            }
            // JACK lists its devices itself (see `jack`), and only while a server runs.
            if crate::jack::is_jack(id.name()) {
                return crate::jack::probe().map(|s| jack_system(&s, is_default));
            }
            let host = cpal::host_from_id(id).ok()?;
            let (def_out, def_in) = (host.default_output_device(), host.default_input_device());
            let outputs = named_devices(&host, true).into_iter().filter_map(|(n, d)| describe(n, &d, def_out.as_ref(), true)).collect();
            let mut inputs: Vec<DeviceInfo> =
                named_devices(&host, false).into_iter().filter_map(|(n, d)| describe(n, &d, def_in.as_ref(), false)).collect();
            // PipeWire: "All sources" first, the default (I/O Setup picks its channels).
            if crate::pw::is_pipewire(id.name()) {
                inputs.iter_mut().for_each(|i| i.is_default = false);
                inputs.insert(0, all_sources_info());
            }
            Some(AudioSystem { name: id.name().to_string(), is_default, outputs, inputs })
        })
        .collect()
}

/// The ASIO system from its drivers' names, without loading any: the running driver (when `live`
/// is on ASIO) carries its live channels, rate and buffer; the others are names, with every
/// standard rate offered and no channel count (0) until they are chosen.
fn asio_system(mut names: Vec<String>, live: Option<&LiveDevice>, is_default: bool) -> AudioSystem {
    let running = live.filter(|l| l.system.eq_ignore_ascii_case("ASIO"));
    if let Some(l) = running
        && !names.contains(&l.output)
    {
        names.push(l.output.clone());
    }
    names.sort();
    names.dedup();
    let info = |name: &str, input: bool| -> DeviceInfo {
        match running.filter(|l| l.output == name) {
            Some(l) => DeviceInfo {
                name: name.to_string(),
                is_default: true,
                max_channels: if input { l.in_channels.unwrap_or(0) } else { l.out_channels },
                sample_rates: vec![l.rate],
                // The driver's own buffer range is not asked for; the standard sizes are offered.
                buffer_min: None,
                buffer_max: None,
            },
            None => DeviceInfo {
                name: name.to_string(),
                is_default: false,
                max_channels: 0,
                sample_rates: STANDARD_RATES.to_vec(),
                buffer_min: None,
                buffer_max: None,
            },
        }
    };
    AudioSystem {
        name: "ASIO".into(),
        is_default,
        outputs: names.iter().map(|n| info(n, false)).collect(),
        // ASIO devices are duplex: the same drivers are the inputs.
        inputs: names.iter().map(|n| info(n, true)).collect(),
    }
}

/// The ASIO drivers installed, by name, as the SDK lists them: the subkeys of
/// `HKEY_LOCAL_MACHINE\SOFTWARE\ASIO`. Nothing is loaded.
#[cfg(windows)]
fn asio_driver_names() -> Vec<String> {
    windows_registry::LOCAL_MACHINE
        .open("SOFTWARE\\ASIO")
        .ok()
        // The iterator borrows the key: collect while it lives.
        .and_then(|key| key.keys().ok().map(|keys| keys.filter(|k| !k.is_empty()).collect::<Vec<String>>()))
        .unwrap_or_default()
}

#[cfg(not(windows))]
fn asio_driver_names() -> Vec<String> {
    Vec::new()
}

/// JACK as the settings window lists it: each device with its port count, at the server's rate
/// and buffer (JACK runs at those; the mix resamples).
fn jack_system(server: &crate::jack::Server, is_default: bool) -> AudioSystem {
    let info = |groups: &[crate::jack::Group]| {
        groups
            .iter()
            .enumerate()
            .map(|(i, g)| DeviceInfo {
                name: g.name.clone(),
                is_default: i == 0,
                max_channels: u16::try_from(g.ports.len()).unwrap_or(u16::MAX),
                sample_rates: vec![server.rate],
                buffer_min: Some(server.buffer),
                buffer_max: Some(server.buffer),
            })
            .collect()
    };
    AudioSystem { name: crate::jack::SYSTEM.to_string(), is_default, outputs: info(&server.sinks), inputs: info(&server.sources) }
}

/// The default audio system's name, without opening it (cpal prefers PipeWire on Linux when its
/// socket is there).
fn default_system() -> String {
    #[cfg(target_os = "linux")]
    if <cpal::platform::PipeWireHost as cpal::traits::HostTrait>::is_available() {
        return "PipeWire".into();
    }
    cpal::default_host().id().name().to_string()
}

/// PipeWire's "All sources" as the settings window lists it: as many channels as there are
/// sources to pick from, at any standard rate (PipeWire converts).
fn all_sources_info() -> DeviceInfo {
    DeviceInfo {
        name: crate::pw::ALL_SOURCES.to_string(),
        is_default: true,
        max_channels: u16::try_from(crate::pw::sources().len()).unwrap_or(u16::MAX).min(64),
        sample_rates: STANDARD_RATES.to_vec(),
        buffer_min: None,
        buffer_max: None,
    }
}

/// The host and device for `cfg`'s "All sources" input: PipeWire's default input, which this
/// host leaves unconnected (`pw::connect` links it).
#[cfg(target_os = "linux")]
pub(crate) fn all_sources_input() -> Option<(cpal::Host, cpal::Device)> {
    let mut host = cpal::platform::PipeWireHost::new().ok()?;
    host.set_connect_automatically(false);
    let host: cpal::Host = host.into();
    let device = host.default_input_device()?;
    Some((host, device))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn all_sources_input() -> Option<(cpal::Host, cpal::Device)> {
    None
}

/// The name of the input device `name` chooses (else the system default) on `host`, without
/// opening it; `None` when there is none.
pub(crate) fn input_device_name(host: &cpal::Host, name: Option<&str>) -> Option<String> {
    if crate::pw::is_pipewire(host.id().name()) && name.is_none_or(|n| n == crate::pw::ALL_SOURCES) {
        return Some(crate::pw::ALL_SOURCES.to_string());
    }
    if crate::jack::is_jack(host.id().name()) {
        return crate::jack::probe().and_then(|s| crate::jack::pick(&s.sources, name).map(|g| g.name.clone()));
    }
    device(host, name, false).and_then(|d| device_name(host, &d, false))
}

/// The per-machine record offset's key (see the UI's preferences): `"{system}|{device}|{buffer}|{rate}"`,
/// the buffer `default` when the device chose it. Drivers add hidden latency that changes with
/// each of these, so a measurement holds only for its own key. "default" is its own key, apart
/// from an explicit buffer of the same size; a device's default buffer may change size under it
/// (WASAPI's can), so measure again after changing the device's own settings.
pub fn latency_key(system: &str, device: &str, buffer: Option<u32>, rate: u32) -> String {
    let buffer = buffer.map_or("default".to_string(), |b| b.to_string());
    format!("{system}|{device}|{buffer}|{rate}")
}

/// The chosen audio system, else the default one.
pub(crate) fn host(cfg: &AudioConfig) -> cpal::Host {
    cfg.system
        .as_deref()
        .and_then(|want| cpal::available_hosts().into_iter().find(|id| id.name().eq_ignore_ascii_case(want)))
        // JACK only with a server running: cpal's JACK host would come up with no devices.
        .filter(|id| !crate::jack::is_jack(id.name()) || crate::jack::probe().is_some())
        .and_then(|id| cpal::host_from_id(id).ok())
        .unwrap_or_else(|| {
            if let Some(s) = &cfg.system {
                log::warn!("audio system `{s}` is not available; using the default");
            }
            cpal::default_host()
        })
}

/// The named output (or input) device of `host`, else its default device. On JACK there is one
/// cpal device each way, whatever the name: `jack::Route` connects it to the named device.
pub(crate) fn device(host: &cpal::Host, name: Option<&str>, output: bool) -> Option<cpal::Device> {
    if crate::jack::is_jack(host.id().name()) {
        return if output { host.default_output_device() } else { host.default_input_device() };
    }
    if let Some(want) = name {
        let found = named_device(host, want, output);
        if found.is_some() {
            return found;
        }
        log::warn!("audio device `{want}` is not available; using the default");
    }
    if output { host.default_output_device() } else { host.default_input_device() }
}

/// The input (or output) device of `host` named exactly `name`; `None` when there is none (no
/// fallback to the default).
pub(crate) fn named_device(host: &cpal::Host, name: &str, output: bool) -> Option<cpal::Device> {
    if crate::jack::is_jack(host.id().name()) {
        let server = crate::jack::probe()?;
        let groups = if output { &server.sinks } else { &server.sources };
        return groups.iter().any(|g| g.name == name).then(|| device(host, None, output)).flatten();
    }
    named_devices(host, output).into_iter().find(|(n, _)| n == name).map(|(_, d)| d)
}

/// The buffer to request: the chosen size when the device's range allows it, else the default.
pub(crate) fn buffer_size(want: Option<u32>, supported: &cpal::SupportedBufferSize) -> cpal::BufferSize {
    match (want, supported) {
        (Some(n), cpal::SupportedBufferSize::Range { min, max }) if (*min..=*max).contains(&n) => cpal::BufferSize::Fixed(n),
        // The device does not say: ask anyway (cpal reports an error if it cannot, and the caller
        // falls back to the default).
        (Some(n), cpal::SupportedBufferSize::Unknown) if (16..=8192).contains(&n) => cpal::BufferSize::Fixed(n),
        _ => cpal::BufferSize::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_tolerates_missing_fields() {
        let c = AudioConfig {
            system: Some("WASAPI".into()),
            output: Some("Speakers".into()),
            input: None,
            sample_rate: Some(48_000),
            buffer_size: Some(256),
            input_sources: vec!["weir-stream:capture_FL".into()],
        };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<AudioConfig>(&s).unwrap(), c);
        assert_eq!(serde_json::from_str::<AudioConfig>("{}").unwrap(), AudioConfig::default());
        assert_eq!(serde_json::from_str::<AudioConfig>(r#"{"output":"X","unknown":1}"#).unwrap().output.as_deref(), Some("X"));
    }

    #[test]
    fn the_asio_list_names_every_driver_and_describes_only_the_running_one() {
        let live = LiveDevice {
            system: "ASIO".into(),
            output: "Steinberg built-in ASIO Driver".into(),
            input: Some("Steinberg built-in ASIO Driver".into()),
            rate: 48_000,
            out_channels: 2,
            in_channels: Some(2),
            buffer: Some(480),
        };
        let names = vec!["Focusrite USB ASIO".to_string(), "Steinberg built-in ASIO Driver".to_string(), "Focusrite USB ASIO".to_string()];
        let s = asio_system(names, Some(&live), false);
        assert_eq!(s.name, "ASIO");
        assert_eq!(s.outputs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Focusrite USB ASIO", "Steinberg built-in ASIO Driver"]);
        let running = &s.outputs[1];
        assert!(running.is_default && running.max_channels == 2 && running.sample_rates == [48_000]);
        assert_eq!(running.buffer_sizes().len(), STANDARD_BUFFERS.len(), "the driver's buffer range is not probed");
        let other = &s.outputs[0];
        assert!(!other.is_default && other.max_channels == 0 && other.sample_rates == STANDARD_RATES && other.buffer_min.is_none());
        assert_eq!(s.inputs[1].max_channels, 2);
        // A running driver the registry doesn't list (a portable install) is still listed.
        let s = asio_system(vec![], Some(&live), true);
        assert_eq!(s.outputs.len(), 1);
        assert!(s.is_default);
        // With the player on WASAPI, every ASIO driver is a name.
        let wasapi = LiveDevice { system: "WASAPI".into(), ..live };
        let s = asio_system(vec!["X".into()], Some(&wasapi), false);
        assert!(!s.outputs[0].is_default && s.outputs[0].max_channels == 0);
    }

    #[test]
    fn buffer_sizes_follow_the_device_range() {
        let r = cpal::SupportedBufferSize::Range { min: 64, max: 1024 };
        assert_eq!(buffer_size(Some(256), &r), cpal::BufferSize::Fixed(256));
        assert_eq!(buffer_size(Some(32), &r), cpal::BufferSize::Default);
        assert_eq!(buffer_size(Some(4096), &r), cpal::BufferSize::Default);
        assert_eq!(buffer_size(None, &r), cpal::BufferSize::Default);
        assert_eq!(buffer_size(Some(128), &cpal::SupportedBufferSize::Unknown), cpal::BufferSize::Fixed(128));
        assert_eq!(buffer_size(Some(1 << 20), &cpal::SupportedBufferSize::Unknown), cpal::BufferSize::Default);
        let d = DeviceInfo {
            name: "x".into(),
            is_default: true,
            max_channels: 2,
            sample_rates: vec![48_000],
            buffer_min: Some(64),
            buffer_max: Some(1024),
        };
        assert_eq!(d.buffer_sizes(), [64, 128, 256, 512, 1024]);
        let open = DeviceInfo { buffer_min: None, buffer_max: None, ..d };
        assert_eq!(open.buffer_sizes().len(), STANDARD_BUFFERS.len());
    }

    #[test]
    fn unknown_systems_and_devices_fall_back_to_the_defaults() {
        let cfg = AudioConfig { system: Some("No Such System".into()), ..Default::default() };
        assert_eq!(host(&cfg).id(), cpal::default_host().id());
        // No device with this name exists: the default (or none, on a machine without audio).
        let h = cpal::default_host();
        let fallback = device(&h, Some("No Such Device 1234"), true);
        assert_eq!(fallback, h.default_output_device());
    }

    #[test]
    fn devices_sharing_a_name_are_told_apart_by_their_driver_id() {
        let item = |n: &str, d: Option<&str>| (n.to_string(), d.map(str::to_string));
        let names = unique_names(&[
            item("HDA Intel PCH", Some("hw:CARD=PCH,DEV=0")),
            item("HDA Intel PCH", Some("plughw:CARD=PCH,DEV=0")),
            item("Speakers (Realtek)", Some("Speakers (Realtek)")),
            item("default", None),
        ]);
        assert_eq!(names, ["HDA Intel PCH (hw:CARD=PCH,DEV=0)", "HDA Intel PCH (plughw:CARD=PCH,DEV=0)", "Speakers (Realtek)", "default"]);
    }

    #[test]
    fn without_a_jack_server_jack_is_not_listed_and_a_jack_choice_falls_back() {
        if crate::jack::probe().is_some() {
            println!("SKIPPED: a JACK server is running");
            return;
        }
        assert!(!audio_systems().iter().any(|s| crate::jack::is_jack(&s.name)));
        let cfg = AudioConfig { system: Some("JACK".into()), ..Default::default() };
        assert_eq!(host(&cfg).id(), cpal::default_host().id());
    }

    #[test]
    fn latency_keys_name_system_device_buffer_and_rate() {
        assert_eq!(latency_key("ASIO", "Focusrite USB ASIO", Some(128), 48_000), "ASIO|Focusrite USB ASIO|128|48000");
        assert_eq!(latency_key("WASAPI", "Speakers", None, 44_100), "WASAPI|Speakers|default|44100");
        assert_ne!(latency_key("ASIO", "X32", Some(96), 48_000), latency_key("ASIO", "X32", Some(128), 48_000));
    }

    #[test]
    fn enumeration_does_not_fail() {
        let systems = audio_systems();
        // Every listed system has a name; on a machine without audio the lists are just empty.
        assert!(systems.iter().all(|s| !s.name.is_empty()));
        assert!(systems.iter().filter(|s| s.is_default).count() <= 1);
    }
}
