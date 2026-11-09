//! Setup › X32 Control: a Behringer X32 (or Midas M32) moves this session's faders through one
//! bus's sends (spec `docs/superpowers/specs/2026-11-09-x32-bus-control-design.md`).
//!
//! The link lives in the network state ([`crate::net_ui::NetState`]); once a frame [`step`]
//! starts or stops it as the settings say and puts the console's moves into the session. While
//! this PC mixes another one remotely, the moves go to that host like any fader move here.

use crate::SoundApp;
use crate::net_look::{Icon, avatar, pill, section, switch, tile};
use crate::theme::Tokens;
use egui::{Color32, vec2};
use serde_json::{Value, json};
use soundcraft_net::x32::{X32Config, X32Link};
use std::time::{Duration, Instant};

/// The console is "connected" while it was heard from this recently (it answers `/info` every 8 s).
const ALIVE: Duration = Duration::from_secs(20);

/// Once a frame: the link as the settings say, and the console's moves into the session.
pub fn step(app: &mut SoundApp) {
    let Some(net) = app.net.as_mut() else { return };
    let cfg = net.hub.config().x32;
    let want = cfg.enabled && !cfg.address.trim().is_empty();
    match (&net.x32, want) {
        (Some(l), true) if l.address() == cfg.address.trim() => l.set_bus(cfg.osc_bus(), cfg.stereo),
        (_, true) => net.x32 = Some(X32Link::start(&cfg.address, cfg.osc_bus(), cfg.stereo, net.hub.waker())),
        (Some(_), false) => net.x32 = None,
        (None, false) => {}
    }
    let Some(link) = &net.x32 else { return };
    let moves = link.drain();
    if moves.is_empty() {
        return;
    }
    let mut apply = std::mem::take(&mut net.x32_apply);
    apply.apply(&mut app.engine, &moves, cfg.master, Instant::now());
    if let Some(net) = app.net.as_mut() {
        net.x32_apply = apply;
    }
}

/// `setup.x32`: `{enabled?, address?, bus?: 1..16, stereo?: bool, master?: bool}` sets what is
/// given (a stereo pair by either of its buses); returns the settings and the link: `{enabled,
/// address, bus, stereo, controller, master, connected, console, error}`.
pub fn command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let Some(net) = app.net.as_ref() else { return Err("the network is not ready yet".into()) };
    if let Some(b) = p.get("bus").and_then(Value::as_u64)
        && !(1..=16).contains(&b)
    {
        return Err("the bus is 1 to 16".into());
    }
    net.hub.update_config(|c| {
        if let Some(v) = p.get("enabled").and_then(Value::as_bool) {
            c.x32.enabled = v;
        }
        if let Some(v) = p.get("address").and_then(Value::as_str) {
            c.x32.address = v.trim().to_string();
        }
        if let Some(v) = p.get("bus").and_then(Value::as_u64).and_then(|b| u8::try_from(b).ok()) {
            c.x32.bus = v;
        }
        if let Some(v) = p.get("master").and_then(Value::as_bool) {
            c.x32.master = v;
        }
        if let Some(v) = p.get("stereo").and_then(Value::as_bool) {
            c.x32.stereo = v;
        }
        c.x32.bus = c.x32.osc_bus();
    });
    step(app);
    let Some(net) = app.net.as_ref() else { return Err("the network is not ready yet".into()) };
    let cfg = net.hub.config().x32;
    let st = net.x32.as_ref().map(X32Link::state).unwrap_or_default();
    Ok(json!({
        "enabled": cfg.enabled,
        "address": cfg.address,
        "bus": cfg.bus,
        "stereo": cfg.stereo,
        "controller": cfg.label(),
        "master": cfg.master,
        "connected": st.last_heard.is_some_and(|t| t.elapsed() < ALIVE),
        "console": st.console,
        "error": st.error,
    }))
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_x32;
    if !open {
        return;
    }
    let t = Tokens::current();
    let Some(net) = app.net.as_mut() else { return };
    let cfg = net.hub.config().x32;
    let st = net.x32.as_ref().map(X32Link::state).unwrap_or_default();
    let alive = st.last_heard.is_some_and(|h| h.elapsed() < ALIVE);
    let strips: Vec<String> = {
        let s = app.engine.session();
        soundcraft_net::x32::strip_tracks(s).iter().filter_map(|id| s.track(*id).map(|t| t.name.clone())).collect()
    };
    let mut set: Option<X32Config> = None;
    egui::Window::new("X32 Control")
        .open(&mut open)
        .default_size(vec2(620.0, 640.0))
        .min_width(540.0)
        .resizable(true)
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.window_bg).inner_margin(egui::Margin::same(18)))
        .show(ctx, |ui| {
            let mut c = cfg.clone();
            // The console and whether it answers.
            ui.horizontal(|ui| {
                avatar(ui, 46.0, Icon::Faders, if alive { t.accent } else { t.button }, Color32::WHITE);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("X32 as a controller").size(20.0).color(t.text));
                    let (line, col) = match (&st.error, alive, cfg.enabled) {
                        (_, _, false) => ("Off".to_string(), t.text_dim),
                        (Some(e), _, _) => (e.clone(), t.rec),
                        (None, true, _) => (format!("Connected to {} at {}", st.console.clone().unwrap_or_else(|| "the console".into()), cfg.address), crate::net_look::LAN),
                        (None, false, _) => (format!("Looking for the console at {}…", cfg.address), crate::net_look::RELAY),
                    };
                    ui.label(egui::RichText::new(line).size(12.5).color(col));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if switch(ui, &mut c.enabled, "Follow the console").changed() && c.enabled && c.address.trim().is_empty() {
                        c.enabled = false;
                    }
                });
            });
            section(ui, "The console");
            tile(ui, false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Address");
                    let r = ui.add(egui::TextEdit::singleline(&mut c.address).hint_text("192.168.1.50").desired_width(170.0));
                    if r.lost_focus() && !c.address.trim().is_empty() && !cfg.enabled {
                        c.enabled = true;
                    }
                    ui.label(egui::RichText::new("the X32's IP (Setup › Network on the desk)").size(11.5).color(t.text_dim));
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Controller");
                    if let Some(i) = crate::net_look::segmented(ui, ["Mono bus", "Stereo pair"], usize::from(c.stereo)) {
                        c.stereo = i == 1;
                        c.bus = c.osc_bus();
                    }
                    let pick = egui::ComboBox::from_id_salt("x32-bus").selected_text(c.label()).width(120.0);
                    pick.show_ui(ui, |ui| {
                        if c.stereo {
                            for b in (1..=15u8).step_by(2) {
                                let mut x = c.clone();
                                x.bus = b;
                                if ui.selectable_label(c.osc_bus() == b, x.label()).clicked() {
                                    c.bus = b;
                                }
                            }
                        } else {
                            for b in 1..=16u8 {
                                ui.selectable_value(&mut c.bus, b, format!("Bus {b}"));
                            }
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(if c.stereo {
                        "Link the two buses on the desk. Each channel's send level moves its track's volume and its send pan moves the track's pan. Use a pair patched to no output, so the room never hears it."
                    } else {
                        "Each channel's send level moves its track's volume. Use a bus patched to no output, so the room never hears it."
                    })
                    .size(11.5)
                    .color(t.text_dim),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    switch(ui, &mut c.master, "Bus master moves the master");
                    ui.label(format!("{}'s master fader moves the session's master", c.label()));
                });
            });
            section(ui, "On the desk");
            tile(ui, false, |ui| {
                for (n, text) in [
                    (1, format!("Press Sends on Fader and select {}.", c.label())),
                    (2, "The channel faders now move this session's tracks. The room's mix stays as it is.".to_string()),
                    (3, "Leave Sends on Fader to hand the faders back to the live mix.".to_string()),
                ] {
                    ui.horizontal(|ui| {
                        pill(ui, &n.to_string(), Color32::WHITE, t.accent);
                        ui.label(text);
                    });
                    ui.add_space(2.0);
                }
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "The mute buttons stay the room's mutes in Sends on Fader: mute here, not on the desk. Moves on the desk are written as automation like any fader move.",
                    )
                    .size(11.5)
                    .color(t.text_dim),
                );
            });
            section(ui, "Channels");
            if strips.is_empty() {
                ui.label(egui::RichText::new("No tracks yet: channel 1 moves the first track from the top, and so on.").size(12.0).color(t.text_dim));
            } else {
                egui::Frame::new().fill(t.panel_bg2).corner_radius(10.0).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::Grid::new("x32-strips").num_columns(4).spacing(vec2(18.0, 4.0)).show(ui, |ui| {
                        for (i, name) in strips.iter().take(32).enumerate() {
                            ui.label(egui::RichText::new(format!("Ch {:>2}", i + 1)).monospace().size(11.5).color(t.text_dim));
                            ui.label(name);
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                    if strips.len() > 32 {
                        ui.label(egui::RichText::new(format!("Tracks past the 32nd ({} more) aren't on the desk.", strips.len() - 32)).size(11.5).color(t.text_dim));
                    }
                });
            }
            if c.master {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(format!("{} master → the session's master", c.label())).size(11.5).color(t.text_dim));
            }
            if st.moves > 0 {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(format!("{} moves from the desk so far", st.moves)).size(11.5).color(t.text_dim));
            }
            if c != cfg {
                set = Some(c);
            }
        });
    if let Some(c) = set
        && let Some(net) = app.net.as_ref()
    {
        net.hub.update_config(|x| x.x32 = c);
    }
    app.ui.show_x32 = open;
    ctx.request_repaint_after(Duration::from_secs(1));
}
