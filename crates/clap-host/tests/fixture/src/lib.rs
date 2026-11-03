//! Test-only CLAP plugins for soundcraft-clap-host: a gain effect and a "DC synth".
//!
//! - `org.soundcraft.test.gain`: stereo in/out, param 7 "Gain" (0..2, default 1), param 3 "Mode"
//!   (stepped enum A/B/C), hidden param 9, latency 3 samples.
//! - `org.soundcraft.test.synth`: instrument, stereo out, CLAP note port; outputs a constant equal
//!   to the velocity of the last note-on plus its MIDI channel (0–15), switching at the event's
//!   sample offset, and 0 once every held (channel, key) has had its note-off. A MIDI CC 7
//!   (volume) sets the constant to its value / 127, a MIDI program change to program + channel / 16.
//!
//! - `org.soundcraft.test.wide16`, `.wide17ports`, `.wide65ch`: effects advertising wide layouts
//!   (16 ports of 64 channels, 17 ports of 2 channels, one port of 65 channels per direction).
//!   `process` writes 1.0 into every output channel it was given when the host's buffers match the
//!   advertised layout, -1.0 when they do not.
//!
//! The gain plugin also has two presets inside the plugin, found through
//! `clap.preset-discovery-factory` (load keys `loud`: gain 2, `quiet`: gain 0.5, both for the
//! gain only, plus `shared` without a plugin id) and loaded through `clap.preset-load`.
//!
//! The gain plugin registers a 20 ms host timer (`clap.timer-support`) at `init` when the host
//! offers one; its read-only param 15 "Ticks" counts the `on_timer` calls.
//!
//! The gain plugin also implements `clap.state` (gain and mode as two little-endian f64s) and a
//! floating-only `clap.gui` without a real window: showing it "edits" the gain to 0.25 in the GUI,
//! reported as an output parameter event by the next `process`.
#![allow(clippy::missing_safety_doc, non_upper_case_globals)]

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::gui::*;
use clap_sys::ext::state::*;
use clap_sys::ext::timer_support::*;
use clap_sys::stream::*;
use clap_sys::ext::latency::*;
use clap_sys::ext::log::*;
use clap_sys::ext::note_ports::*;
use clap_sys::ext::params::*;
use clap_sys::ext::preset_load::*;
use clap_sys::factory::plugin_factory::*;
use clap_sys::factory::preset_discovery::*;
use clap_sys::universal_plugin_id::clap_universal_plugin_id;
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::*;
use clap_sys::process::*;
use clap_sys::version::CLAP_VERSION;
use std::ffi::{CStr, c_char, c_void};

struct Features<const N: usize>([*const c_char; N]);
unsafe impl<const N: usize> Sync for Features<N> {}

static GAIN_FEATURES: Features<3> = Features([c"audio-effect".as_ptr(), c"stereo".as_ptr(), std::ptr::null()]);
static SYNTH_FEATURES: Features<3> = Features([c"instrument".as_ptr(), c"synthesizer".as_ptr(), std::ptr::null()]);

static WIDE_FEATURES: Features<2> = Features([c"audio-effect".as_ptr(), std::ptr::null()]);

const fn wide_desc(id: &'static CStr, name: &'static CStr) -> clap_plugin_descriptor {
    clap_plugin_descriptor {
        clap_version: CLAP_VERSION,
        id: id.as_ptr(),
        name: name.as_ptr(),
        vendor: c"SoundCraft tests".as_ptr(),
        url: std::ptr::null(),
        manual_url: std::ptr::null(),
        support_url: std::ptr::null(),
        version: c"1.0.0".as_ptr(),
        description: std::ptr::null(),
        features: WIDE_FEATURES.0.as_ptr(),
    }
}

static WIDE16_DESC: clap_plugin_descriptor = wide_desc(c"org.soundcraft.test.wide16", c"Fixture Wide 16x64");
static WIDE17_DESC: clap_plugin_descriptor = wide_desc(c"org.soundcraft.test.wide17ports", c"Fixture Wide 17 ports");
static WIDE65_DESC: clap_plugin_descriptor = wide_desc(c"org.soundcraft.test.wide65ch", c"Fixture Wide 65 channels");

static GAIN_DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c"org.soundcraft.test.gain".as_ptr(),
    name: c"Fixture Gain".as_ptr(),
    vendor: c"SoundCraft tests".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"test gain".as_ptr(),
    features: GAIN_FEATURES.0.as_ptr(),
};

static SYNTH_DESC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c"org.soundcraft.test.synth".as_ptr(),
    name: c"Fixture Synth".as_ptr(),
    vendor: c"SoundCraft tests".as_ptr(),
    url: std::ptr::null(),
    manual_url: std::ptr::null(),
    support_url: std::ptr::null(),
    version: c"1.0.0".as_ptr(),
    description: std::ptr::null(),
    features: SYNTH_FEATURES.0.as_ptr(),
};

struct Fx {
    plugin: clap_plugin,
    host: *const clap_host,
    synth: bool,
    /// `(ports per direction, channels per port)` for the wide fixtures.
    wide: Option<(u32, u32)>,
    gain: f64,
    mode: f64,
    level: f64,
    /// Held notes: a channel bitmask per key.
    keys: [u16; 128],
    active: bool,
    processing: bool,
    gui_created: bool,
    gui_visible: bool,
    gui_edit_pending: bool,
    /// `on_timer` calls (param 15).
    ticks: f64,
}

unsafe fn fx<'a>(p: *const clap_plugin) -> &'a mut Fx {
    unsafe { &mut *((*p).plugin_data as *mut Fx) }
}

unsafe extern "C" fn init(p: *const clap_plugin) -> bool {
    let f = unsafe { fx(p) };
    // Exercise the host's extension lookup + log callback.
    let host = unsafe { &*f.host };
    if let Some(ge) = host.get_extension {
        let log = unsafe { ge(f.host, CLAP_EXT_LOG.as_ptr()) } as *const clap_host_log;
        if let Some(l) = unsafe { log.as_ref() }.and_then(|l| l.log) {
            unsafe { l(f.host, CLAP_LOG_INFO, c"fixture init".as_ptr()) };
        }
        let none = unsafe { ge(f.host, c"clap.does-not-exist".as_ptr()) };
        if !none.is_null() {
            return false;
        }
        if !f.synth {
            let timers = unsafe { ge(f.host, CLAP_EXT_TIMER_SUPPORT.as_ptr()) } as *const clap_host_timer_support;
            if let Some(register) = unsafe { timers.as_ref() }.and_then(|t| t.register_timer) {
                let mut id: clap_id = 0;
                assert!(unsafe { register(f.host, 20, &mut id) }, "the host registers a timer");
            }
        }
    }
    true
}

unsafe extern "C" fn destroy(p: *const clap_plugin) {
    assert!(!unsafe { fx(p) }.gui_created, "plugin destroyed with its GUI alive");
    let data = unsafe { (*p).plugin_data } as *mut Fx;
    drop(unsafe { Box::from_raw(data) });
}

unsafe extern "C" fn activate(p: *const clap_plugin, sr: f64, min: u32, max: u32) -> bool {
    let f = unsafe { fx(p) };
    if f.active || !(sr > 0.0) || min == 0 || max < min {
        return false;
    }
    f.active = true;
    true
}

unsafe extern "C" fn deactivate(p: *const clap_plugin) {
    let f = unsafe { fx(p) };
    assert!(!f.processing, "deactivate while processing");
    f.active = false;
}

unsafe extern "C" fn start_processing(p: *const clap_plugin) -> bool {
    let f = unsafe { fx(p) };
    assert!(f.active, "start_processing while inactive");
    f.processing = true;
    true
}

unsafe extern "C" fn stop_processing(p: *const clap_plugin) {
    unsafe { fx(p) }.processing = false;
}

unsafe extern "C" fn reset(p: *const clap_plugin) {
    let f = unsafe { fx(p) };
    f.level = 0.0;
    f.keys = [0; 128];
}

fn apply(f: &mut Fx, h: &clap_event_header) {
    if h.space_id != CLAP_CORE_EVENT_SPACE_ID {
        return;
    }
    match h.type_ {
        CLAP_EVENT_PARAM_VALUE => {
            let e = unsafe { &*(h as *const clap_event_header as *const clap_event_param_value) };
            match e.param_id {
                7 => f.gain = e.value,
                3 => f.mode = e.value,
                _ => {}
            }
        }
        CLAP_EVENT_NOTE_ON => {
            let e = unsafe { &*(h as *const clap_event_header as *const clap_event_note) };
            if (0..128).contains(&e.key) && (0..16).contains(&e.channel) {
                f.keys[e.key as usize] |= 1 << e.channel;
                f.level = e.velocity + f64::from(e.channel);
            }
        }
        CLAP_EVENT_NOTE_OFF => {
            let e = unsafe { &*(h as *const clap_event_header as *const clap_event_note) };
            if (0..128).contains(&e.key) && (0..16).contains(&e.channel) {
                f.keys[e.key as usize] &= !(1 << e.channel);
            }
            if f.keys.iter().all(|&k| k == 0) {
                f.level = 0.0;
            }
        }
        CLAP_EVENT_MIDI => {
            let e = unsafe { &*(h as *const clap_event_header as *const clap_event_midi) };
            if e.data[0] & 0xF0 == 0xB0 && e.data[1] == 7 {
                f.level = f64::from(e.data[2]) / 127.0;
            } else if e.data[0] & 0xF0 == 0xC0 {
                f.level = f64::from(e.data[1]) + f64::from(e.data[0] & 0x0F) / 16.0;
            }
        }
        _ => {}
    }
}

unsafe extern "C" fn process(p: *const clap_plugin, pr: *const clap_process) -> clap_process_status {
    let f = unsafe { fx(p) };
    let pr = unsafe { &*pr };
    assert!(f.processing, "process while not processing");
    let n = pr.frames_count as usize;
    let ev = unsafe { &*pr.in_events };
    let count = unsafe { (ev.size.unwrap())(pr.in_events) };
    if let Some((ports, channels)) = f.wide {
        let fits = |bufs: *const clap_audio_buffer, n: u32| {
            n == ports && (0..n as usize).all(|i| unsafe { &*bufs.add(i) }.channel_count == channels)
        };
        let ok = fits(pr.audio_inputs, pr.audio_inputs_count) && fits(pr.audio_outputs, pr.audio_outputs_count);
        if pr.audio_outputs_count > 0 {
            let out = unsafe { &*pr.audio_outputs };
            for c in 0..out.channel_count as usize {
                let o = unsafe { *out.data32.add(c) };
                for i in 0..n {
                    unsafe { *o.add(i) = if ok { 1.0 } else { -1.0 } };
                }
            }
        }
        return CLAP_PROCESS_CONTINUE;
    }
    if pr.audio_outputs_count != 1 {
        return CLAP_PROCESS_ERROR;
    }
    let out: &clap_audio_buffer = unsafe { &*pr.audio_outputs };
    let input: Option<&clap_audio_buffer> = if f.synth {
        if pr.audio_inputs_count != 0 {
            return CLAP_PROCESS_ERROR;
        }
        None
    } else {
        if pr.audio_inputs_count != 1 {
            return CLAP_PROCESS_ERROR;
        }
        Some(unsafe { &*pr.audio_inputs })
    };
    if f.gui_edit_pending {
        f.gui_edit_pending = false;
        f.gain = 0.25;
        let e = clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: 7,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value: 0.25,
        };
        let out = unsafe { &*pr.out_events };
        assert!(unsafe { (out.try_push.unwrap())(pr.out_events, &e.header) });
        // Other event types are accepted and ignored by the host.
        let note = clap_event_note {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_note>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_NOTE_END,
                flags: 0,
            },
            note_id: -1,
            port_index: 0,
            channel: 0,
            key: 60,
            velocity: 0.0,
        };
        assert!(unsafe { (out.try_push.unwrap())(pr.out_events, &note.header) });
    }
    let mut next = 0u32;
    let mut last_time = 0u32;
    for i in 0..n {
        while next < count {
            let h = unsafe { &*(ev.get.unwrap())(pr.in_events, next) };
            assert!(h.time >= last_time, "events not sorted");
            assert!((h.time as usize) < n, "event time outside the block");
            if h.time as usize > i {
                break;
            }
            last_time = h.time;
            apply(f, h);
            next += 1;
        }
        for c in 0..out.channel_count as usize {
            let o = unsafe { *out.data32.add(c) };
            let v = match input {
                Some(inp) => {
                    let x = unsafe { *(*inp.data32.add(c.min(inp.channel_count as usize - 1))).add(i) };
                    x * f.gain as f32
                }
                None => f.level as f32,
            };
            unsafe { *o.add(i) = v };
        }
    }
    CLAP_PROCESS_CONTINUE
}

// ---- extensions ----

unsafe extern "C" fn params_count(p: *const clap_plugin) -> u32 {
    if unsafe { fx(p) }.synth { 0 } else { 4 }
}

fn write_name(dst: &mut [c_char], s: &CStr) {
    for (d, b) in dst.iter_mut().zip(s.to_bytes_with_nul()) {
        *d = *b as c_char;
    }
}

unsafe extern "C" fn params_get_info(_p: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool {
    let info = unsafe { &mut *info };
    let (id, name, flags, min, max, def): (clap_id, &CStr, u32, f64, f64, f64) = match index {
        0 => (7, c"Gain", CLAP_PARAM_IS_AUTOMATABLE, 0.0, 2.0, 1.0),
        1 => (3, c"Mode", CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_ENUM, 0.0, 2.0, 0.0),
        2 => (9, c"Secret", CLAP_PARAM_IS_HIDDEN, 0.0, 1.0, 0.0),
        3 => (15, c"Ticks", CLAP_PARAM_IS_READONLY, 0.0, 1_000_000.0, 0.0),
        _ => return false,
    };
    info.id = id;
    info.flags = flags;
    info.cookie = std::ptr::null_mut();
    write_name(&mut info.name, name);
    write_name(&mut info.module, c"");
    info.min_value = min;
    info.max_value = max;
    info.default_value = def;
    true
}

unsafe extern "C" fn params_get_value(p: *const clap_plugin, id: clap_id, out: *mut f64) -> bool {
    let f = unsafe { fx(p) };
    let v = match id {
        7 => f.gain,
        3 => f.mode,
        9 => 0.0,
        15 => f.ticks,
        _ => return false,
    };
    unsafe { *out = v };
    true
}

unsafe extern "C" fn params_value_to_text(_p: *const clap_plugin, id: clap_id, v: f64, buf: *mut c_char, cap: u32) -> bool {
    if id != 3 || cap < 2 {
        return false;
    }
    let label = [b'A', b'B', b'C'].get(v.round() as usize).copied().unwrap_or(b'?');
    unsafe {
        *buf = label as c_char;
        *buf.add(1) = 0;
    }
    true
}

unsafe extern "C" fn params_flush(p: *const clap_plugin, inp: *const clap_input_events, _out: *const clap_output_events) {
    let f = unsafe { fx(p) };
    let ev = unsafe { &*inp };
    let n = unsafe { (ev.size.unwrap())(inp) };
    for i in 0..n {
        apply(f, unsafe { &*(ev.get.unwrap())(inp, i) });
    }
}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: None,
    flush: Some(params_flush),
};

unsafe extern "C" fn ports_count(p: *const clap_plugin, is_input: bool) -> u32 {
    if let Some((ports, _)) = unsafe { fx(p) }.wide {
        return ports;
    }
    if is_input && unsafe { fx(p) }.synth { 0 } else { 1 }
}

unsafe extern "C" fn ports_get(p: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool {
    if let Some((ports, channels)) = unsafe { fx(p) }.wide {
        if index >= ports {
            return false;
        }
        let info = unsafe { &mut *info };
        info.id = index * 2 + u32::from(is_input);
        write_name(&mut info.name, c"wide");
        info.flags = if index == 0 { CLAP_AUDIO_PORT_IS_MAIN } else { 0 };
        info.channel_count = channels;
        info.port_type = std::ptr::null();
        info.in_place_pair = clap_sys::id::CLAP_INVALID_ID;
        return true;
    }
    if index != 0 || (is_input && unsafe { fx(p) }.synth) {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = if is_input { 0 } else { 1 };
    write_name(&mut info.name, c"main");
    info.flags = CLAP_AUDIO_PORT_IS_MAIN;
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = clap_sys::id::CLAP_INVALID_ID;
    true
}

static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports { count: Some(ports_count), get: Some(ports_get) };

unsafe extern "C" fn latency_get(_p: *const clap_plugin) -> u32 {
    3
}

static LATENCY: clap_plugin_latency = clap_plugin_latency { get: Some(latency_get) };

unsafe extern "C" fn notes_count(p: *const clap_plugin, is_input: bool) -> u32 {
    if is_input && unsafe { fx(p) }.synth { 1 } else { 0 }
}

unsafe extern "C" fn notes_get(_p: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_note_port_info) -> bool {
    if index != 0 || !is_input {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = 0;
    info.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    write_name(&mut info.name, c"notes");
    true
}

static NOTES: clap_plugin_note_ports = clap_plugin_note_ports { count: Some(notes_count), get: Some(notes_get) };

unsafe extern "C" fn state_save(p: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let f = unsafe { fx(p) };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&f.gain.to_le_bytes());
    bytes[8..].copy_from_slice(&f.mode.to_le_bytes());
    let s = unsafe { &*stream };
    // Write in two pieces to exercise appending.
    let a = unsafe { (s.write.unwrap())(stream, bytes.as_ptr().cast(), 5) };
    let b = unsafe { (s.write.unwrap())(stream, bytes[5..].as_ptr().cast(), 11) };
    a == 5 && b == 11
}

unsafe extern "C" fn state_load(p: *const clap_plugin, stream: *const clap_istream) -> bool {
    let f = unsafe { fx(p) };
    let s = unsafe { &*stream };
    let mut bytes = [0u8; 17];
    let mut got = 0usize;
    loop {
        let n = unsafe { (s.read.unwrap())(stream, bytes[got..].as_mut_ptr().cast(), (17 - got) as u64) };
        if n <= 0 {
            break;
        }
        got += n as usize;
        if got == 17 {
            break;
        }
    }
    if got != 16 {
        return false;
    }
    let gain = f64::from_le_bytes(bytes[..8].try_into().unwrap());
    let mode = f64::from_le_bytes(bytes[8..16].try_into().unwrap());
    if !(0.0..=2.0).contains(&gain) || !(0.0..=2.0).contains(&mode) {
        return false;
    }
    f.gain = gain;
    f.mode = mode;
    true
}

static STATE: clap_plugin_state = clap_plugin_state { save: Some(state_save), load: Some(state_load) };

unsafe extern "C" fn gui_is_api_supported(_p: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    assert!(!api.is_null());
    floating
}

unsafe extern "C" fn gui_create(p: *const clap_plugin, _api: *const c_char, floating: bool) -> bool {
    let f = unsafe { fx(p) };
    assert!(floating && !f.gui_created, "create");
    f.gui_created = true;
    true
}

unsafe extern "C" fn gui_destroy(p: *const clap_plugin) {
    let f = unsafe { fx(p) };
    assert!(f.gui_created, "destroy without create");
    f.gui_created = false;
    f.gui_visible = false;
}

unsafe extern "C" fn gui_set_scale(_p: *const clap_plugin, scale: f64) -> bool {
    scale > 0.0
}

unsafe extern "C" fn gui_suggest_title(p: *const clap_plugin, title: *const c_char) {
    assert!(unsafe { fx(p) }.gui_created);
    assert!(!unsafe { CStr::from_ptr(title) }.to_bytes().is_empty());
}

unsafe extern "C" fn gui_show(p: *const clap_plugin) -> bool {
    let f = unsafe { fx(p) };
    assert!(f.gui_created, "show without create");
    f.gui_visible = true;
    f.gui_edit_pending = true;
    // Exercise the host GUI extension.
    let host = unsafe { &*f.host };
    let hg = unsafe { (host.get_extension.unwrap())(f.host, CLAP_EXT_GUI.as_ptr()) } as *const clap_host_gui;
    let hg = unsafe { hg.as_ref() }.expect("host gui extension");
    assert!(unsafe { (hg.request_resize.unwrap())(f.host, 300, 200) });
    true
}

unsafe extern "C" fn gui_hide(p: *const clap_plugin) -> bool {
    unsafe { fx(p) }.gui_visible = false;
    true
}

unsafe extern "C" fn on_timer(p: *const clap_plugin, _timer_id: clap_id) {
    unsafe { fx(p) }.ticks += 1.0;
}

static TIMER: clap_plugin_timer_support = clap_plugin_timer_support { on_timer: Some(on_timer) };

static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(gui_is_api_supported),
    get_preferred_api: None,
    create: Some(gui_create),
    destroy: Some(gui_destroy),
    set_scale: Some(gui_set_scale),
    get_size: None,
    can_resize: None,
    get_resize_hints: None,
    adjust_size: None,
    set_size: None,
    set_parent: None,
    set_transient: None,
    suggest_title: Some(gui_suggest_title),
    show: Some(gui_show),
    hide: Some(gui_hide),
};

unsafe extern "C" fn get_extension(p: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) };
    let synth = unsafe { fx(p) }.synth;
    if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY && !synth {
        &LATENCY as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS && synth {
        &NOTES as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE && !synth {
        &STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_GUI && !synth {
        &GUI as *const _ as *const c_void
    } else if id == CLAP_EXT_PRESET_LOAD && !synth {
        &PRESET_LOAD as *const _ as *const c_void
    } else if id == CLAP_EXT_TIMER_SUPPORT && !synth {
        &TIMER as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

unsafe extern "C" fn on_main_thread(_p: *const clap_plugin) {}

// ---- presets ----

unsafe extern "C" fn preset_from_location(p: *const clap_plugin, kind: u32, _location: *const c_char, key: *const c_char) -> bool {
    if kind != CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN || key.is_null() {
        return false;
    }
    let f = unsafe { fx(p) };
    match unsafe { CStr::from_ptr(key) }.to_bytes() {
        b"loud" => f.gain = 2.0,
        b"quiet" => f.gain = 0.5,
        b"shared" => f.gain = 1.5,
        _ => return false,
    }
    true
}

static PRESET_LOAD: clap_plugin_preset_load = clap_plugin_preset_load { from_location: Some(preset_from_location) };

struct Shared<T>(T);
unsafe impl<T> Sync for Shared<T> {}

static PROVIDER_DESC: Shared<clap_preset_discovery_provider_descriptor> = Shared(clap_preset_discovery_provider_descriptor {
    clap_version: CLAP_VERSION,
    id: c"org.soundcraft.test.presets".as_ptr(),
    name: c"Fixture presets".as_ptr(),
    vendor: c"SoundCraft tests".as_ptr(),
});

unsafe extern "C" fn provider_init(p: *const clap_preset_discovery_provider) -> bool {
    let indexer = unsafe { (*p).provider_data } as *const clap_preset_discovery_indexer;
    let loc = clap_preset_discovery_location {
        flags: CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT,
        name: c"Built in".as_ptr(),
        kind: CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN,
        location: std::ptr::null(),
    };
    unsafe { ((*indexer).declare_location.unwrap())(indexer, &loc) }
}

unsafe extern "C" fn provider_destroy(p: *const clap_preset_discovery_provider) {
    drop(unsafe { Box::from_raw(p as *mut clap_preset_discovery_provider) });
}

unsafe extern "C" fn provider_get_metadata(
    _p: *const clap_preset_discovery_provider,
    kind: u32,
    _location: *const c_char,
    r: *const clap_preset_discovery_metadata_receiver,
) -> bool {
    assert_eq!(kind, CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN);
    let rv = unsafe { &*r };
    let gain = clap_universal_plugin_id { abi: c"clap".as_ptr(), id: c"org.soundcraft.test.gain".as_ptr() };
    let synth = clap_universal_plugin_id { abi: c"clap".as_ptr(), id: c"org.soundcraft.test.synth".as_ptr() };
    for (name, key, id) in [(c"Loud", c"loud", Some(&gain)), (c"Quiet", c"quiet", Some(&gain)), (c"Synth Pad", c"pad", Some(&synth)), (c"Shared", c"shared", None)] {
        unsafe {
            assert!((rv.begin_preset.unwrap())(r, name.as_ptr(), key.as_ptr()));
            if let Some(id) = id {
                (rv.add_plugin_id.unwrap())(r, id);
            }
            (rv.set_flags.unwrap())(r, CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT);
        }
    }
    true
}

unsafe extern "C" fn provider_get_extension(_p: *const clap_preset_discovery_provider, _id: *const c_char) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn discovery_count(_f: *const clap_preset_discovery_factory) -> u32 {
    1
}

unsafe extern "C" fn discovery_desc(_f: *const clap_preset_discovery_factory, i: u32) -> *const clap_preset_discovery_provider_descriptor {
    if i == 0 { &PROVIDER_DESC.0 } else { std::ptr::null() }
}

unsafe extern "C" fn discovery_create(
    _f: *const clap_preset_discovery_factory,
    indexer: *const clap_preset_discovery_indexer,
    id: *const c_char,
) -> *const clap_preset_discovery_provider {
    if unsafe { CStr::from_ptr(id) } != c"org.soundcraft.test.presets" {
        return std::ptr::null();
    }
    Box::into_raw(Box::new(clap_preset_discovery_provider {
        desc: &PROVIDER_DESC.0,
        provider_data: indexer as *mut c_void,
        init: Some(provider_init),
        destroy: Some(provider_destroy),
        get_metadata: Some(provider_get_metadata),
        get_extension: Some(provider_get_extension),
    }))
}

static DISCOVERY: clap_preset_discovery_factory =
    clap_preset_discovery_factory { count: Some(discovery_count), get_descriptor: Some(discovery_desc), create: Some(discovery_create) };

// ---- factory + entry ----

unsafe extern "C" fn factory_count(_f: *const clap_plugin_factory) -> u32 {
    5
}

unsafe extern "C" fn factory_desc(_f: *const clap_plugin_factory, i: u32) -> *const clap_plugin_descriptor {
    match i {
        0 => &GAIN_DESC,
        1 => &SYNTH_DESC,
        2 => &WIDE16_DESC,
        3 => &WIDE17_DESC,
        4 => &WIDE65_DESC,
        _ => std::ptr::null(),
    }
}

unsafe extern "C" fn factory_create(_f: *const clap_plugin_factory, host: *const clap_host, id: *const c_char) -> *const clap_plugin {
    let id = unsafe { CStr::from_ptr(id) };
    let (desc, synth, wide) = if id == c"org.soundcraft.test.gain" {
        (&GAIN_DESC as *const _, false, None)
    } else if id == c"org.soundcraft.test.synth" {
        (&SYNTH_DESC as *const _, true, None)
    } else if id == c"org.soundcraft.test.wide16" {
        (&WIDE16_DESC as *const _, false, Some((16, 64)))
    } else if id == c"org.soundcraft.test.wide17ports" {
        (&WIDE17_DESC as *const _, false, Some((17, 2)))
    } else if id == c"org.soundcraft.test.wide65ch" {
        (&WIDE65_DESC as *const _, false, Some((1, 65)))
    } else {
        return std::ptr::null();
    };
    let b = Box::new(Fx {
        plugin: clap_plugin {
            desc,
            plugin_data: std::ptr::null_mut(),
            init: Some(init),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(deactivate),
            start_processing: Some(start_processing),
            stop_processing: Some(stop_processing),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(get_extension),
            on_main_thread: Some(on_main_thread),
        },
        host,
        synth,
        wide,
        gain: 1.0,
        mode: 0.0,
        level: 0.0,
        keys: [0; 128],
        active: false,
        processing: false,
        gui_created: false,
        gui_visible: false,
        gui_edit_pending: false,
        ticks: 0.0,
    });
    let raw = Box::into_raw(b);
    unsafe {
        (*raw).plugin.plugin_data = raw as *mut c_void;
        &(*raw).plugin
    }
}

static FACTORY: clap_plugin_factory =
    clap_plugin_factory { get_plugin_count: Some(factory_count), get_plugin_descriptor: Some(factory_desc), create_plugin: Some(factory_create) };

unsafe extern "C" fn entry_init(_path: *const c_char) -> bool {
    true
}
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn entry_get_factory(id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const _ as *const c_void
    } else if id == CLAP_PRESET_DISCOVERY_FACTORY_ID {
        &DISCOVERY as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry =
    clap_plugin_entry { clap_version: CLAP_VERSION, init: Some(entry_init), deinit: Some(entry_deinit), get_factory: Some(entry_get_factory) };
