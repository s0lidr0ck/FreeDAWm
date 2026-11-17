//! The helper process's side (`freedawm-plugin-host`): one plug-in, served over the shared
//! exchange, with requests on stdin and one reply line each on stdout.

use crate::exchange::{Event, Helper, words_for};
use crate::proto::{Info, Param, Prepare, Reply, Request, hex, unhex};
use crate::shm::Shm;
use soundcraft_dsp::{Plugin, Taper, Unit};
use std::io::{BufRead, Write};
use std::sync::mpsc::{TryRecvError, channel};
use std::time::Duration;

/// The plug-in and what it is served through.
struct Served {
    plugin: Box<dyn Plugin>,
    /// Parameter ids by the index the host sends.
    ids: Vec<&'static str>,
    exchange: Option<(Shm, Helper)>,
}

/// Serves requests from stdin until it ends (the host went away) or says `quit`, and the
/// blocks the host sends meanwhile. The process's exit code.
pub fn serve() -> i32 {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut out = std::io::stdout().lock();
    let mut served: Option<Served> = None;
    let mut idle = 0u32;
    loop {
        match rx.try_recv() {
            Ok(line) => {
                let Ok(req) = serde_json::from_str::<Request>(&line) else {
                    let _ = answer(&mut out, Reply { error: Some("not a request".into()), ..Reply::default() });
                    continue;
                };
                if req == Request::Quit {
                    return 0;
                }
                let reply = handle(&mut served, req);
                if answer(&mut out, reply).is_err() {
                    return 1;
                }
                idle = 0;
            }
            Err(TryRecvError::Disconnected) => return 0,
            Err(TryRecvError::Empty) => {}
        }
        let did = served.as_mut().is_some_and(|s| match &mut s.exchange {
            Some((shm, h)) => h.serve(shm.words(), |events, io, n| run(&mut *s.plugin, &s.ids, events, io, n)),
            None => false,
        });
        // Waiting for the next block: spin a little, then yield, then sleep briefly.
        idle = if did { 0 } else { idle.saturating_add(1) };
        if idle > 4_000 {
            std::thread::sleep(Duration::from_micros(200));
        } else if idle > 2_000 {
            std::thread::yield_now();
        } else {
            std::hint::spin_loop();
        }
    }
}

fn answer(out: &mut impl Write, reply: Reply) -> std::io::Result<()> {
    let mut line = serde_json::to_string(&reply).map_err(std::io::Error::other)?;
    line.push('\n');
    out.write_all(line.as_bytes())?;
    out.flush()
}

fn failed(e: impl Into<String>) -> Reply {
    Reply { error: Some(e.into()), ..Reply::default() }
}

fn handle(served: &mut Option<Served>, req: Request) -> Reply {
    match req {
        Request::Describe { id, dir } => match open(&id, dir) {
            Ok(p) => Reply { ok: true, info: Some(info_of(&*p)), ..Reply::default() },
            Err(e) => failed(e),
        },
        Request::Open { id, dir, prepare } => {
            let plugin = match open(&id, dir) {
                Ok(p) => p,
                Err(e) => return failed(e),
            };
            let ids = plugin.info().params.iter().map(|p| p.id).collect();
            let s = served.insert(Served { plugin, ids, exchange: None });
            let mut reply = prepare_with(s, &prepare);
            if reply.ok {
                reply.info = Some(info_of(&*s.plugin));
            }
            reply
        }
        Request::Prepare { prepare } => match served {
            Some(s) => prepare_with(s, &prepare),
            None => failed("no plug-in is open"),
        },
        Request::SaveState => match served.as_mut().map(|s| s.plugin.save_state()) {
            Some(Some(state)) => Reply { ok: true, state: Some(hex(&state)), ..Reply::default() },
            _ => failed("the plug-in did not save its state"),
        },
        Request::LoadState { state } => {
            let took = match (served.as_mut(), unhex(&state)) {
                (Some(s), Some(b)) => s.plugin.load_state(&b),
                _ => false,
            };
            if took { Reply { ok: true, ..Reply::default() } } else { failed("the plug-in did not take the state") }
        }
        Request::Quit => Reply { ok: true, ..Reply::default() },
    }
}

/// Makes plug-in `id`: a `clap:` one (the bundles in `dir` made known first), or for tests a
/// built-in one.
fn open(id: &str, dir: Option<String>) -> Result<Box<dyn Plugin>, String> {
    if let Some(d) = dir.filter(|_| soundcraft_clap_host::parse_id(id).is_some()) {
        soundcraft_clap_host::add_search_dir(std::path::Path::new(&d));
    }
    match soundcraft_clap_host::parse_id(id) {
        Some(_) => soundcraft_clap_host::instantiate(id).map_err(|e| e.to_string()),
        // Built-in plug-ins: the tests' stand-ins.
        None => soundcraft_dsp::create(id).ok_or_else(|| format!("no plug-in `{id}`")),
    }
}

fn prepare_with(s: &mut Served, p: &Prepare) -> Reply {
    s.exchange = None;
    s.plugin.set_offline(p.offline);
    s.plugin.prepare(p.rate, p.max, p.channels);
    let shm = match Shm::open(std::path::Path::new(&p.shm), words_for(p.channels, p.max)) {
        Ok(m) => m,
        Err(e) => return failed(format!("{}: {e}", p.shm)),
    };
    let Some(h) = Helper::new(shm.words()) else { return failed("the shared block is not set up") };
    s.exchange = Some((shm, h));
    Reply { ok: true, latency: s.plugin.latency(), tail: s.plugin.tail_samples(), ..Reply::default() }
}

fn info_of(p: &dyn Plugin) -> Info {
    let i = p.info();
    let params = i
        .params
        .iter()
        .map(|q| Param {
            id: q.id.into(),
            name: q.name.into(),
            min: q.min,
            max: q.max,
            default: q.default,
            unit: serde_json::to_value(q.unit).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            log: q.taper == Taper::Log,
            choices: q.choices.iter().map(|c| c.to_string()).collect(),
            group: q.group.into(),
        })
        .collect();
    Info { name: i.name.into(), short_name: i.short_name.into(), instrument: i.is_instrument, params, category: Some(i.category) }
}

/// The unit a [`Param`] names (`None` for one it does not know).
pub(crate) fn unit_named(name: &str) -> Unit {
    match name {
        "Db" => Unit::Db,
        "Hz" => Unit::Hz,
        "Ms" => Unit::Ms,
        "Percent" => Unit::Percent,
        "Ratio" => Unit::Ratio,
        "Semitones" => Unit::Semitones,
        "Cents" => Unit::Cents,
        "Seconds" => Unit::Seconds,
        "Toggle" => Unit::Toggle,
        "Choice" => Unit::Choice,
        _ => Unit::None,
    }
}

fn run(p: &mut dyn Plugin, ids: &[&'static str], events: &[Event], io: &mut [Vec<f32>], n: usize) {
    for e in events {
        match *e {
            Event::Param { index, value } => {
                if let Some(id) = usize::try_from(index).ok().and_then(|i| ids.get(i)) {
                    p.set_param(id, value);
                }
            }
            Event::Midi { offset, msg } => p.midi_raw(usize::try_from(offset).unwrap_or(0).min(n.saturating_sub(1)), msg),
            Event::AllNotesOff => p.all_notes_off(),
            Event::Reset => p.reset(),
        }
    }
    p.process(io, n);
}
