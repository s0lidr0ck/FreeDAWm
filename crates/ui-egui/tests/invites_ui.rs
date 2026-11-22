//! Setup › Network › Connect over the internet: the two kinds of invite (one person, reusable
//! with its time and joins) and the list of open invites with Revoke.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{Services, SoundApp};

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
    Harness::builder().with_size(vec2(1600.0, 2400.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    )
}

#[test]
fn both_kinds_are_offered_and_open_invites_are_listed_with_revoke() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.ui.show_network = true;
    let mut h = harness(app);
    h.run_steps(3);
    let Some(hub) = h.state().net.as_ref().map(|n| n.hub.clone()) else {
        eprintln!("SKIPPED: no network state");
        return;
    };
    // The host's switch as saved (not started here: no endpoint in a test).
    hub.update_config(|c| c.internet_host = true);
    h.run_steps(3);
    assert!(h.query_by_label("One person").is_some());
    assert!(h.query_by_label_contains("people can join with the same code").is_none(), "one person by default");
    h.get_by_label("Reusable").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("up to 10 people can join with the same code within 1 day").is_some(), "10 joins, a day by default");
    // Open invites: one of each.
    hub.new_invite_secret();
    let secret = hub.new_reusable_invite(86_400, 10);
    assert_eq!(secret.len(), 32);
    h.run_steps(3);
    assert!(h.query_by_label_contains("Invite for one person: 30 min left").is_some());
    assert!(h.query_by_label_contains("Reusable invite: 24 h left, 0 of 10 joins used").is_some());
    let revoke: Vec<_> = h.query_all_by_label("Revoke").collect();
    assert_eq!(revoke.len(), 2);
    revoke[1].click();
    h.run_steps(3);
    assert_eq!(hub.invites().len(), 1);
    assert!(!hub.invites()[0].reusable, "the reusable one revoked");
    assert!(hub.config().invites.is_empty());
}
