//! The X32 link against a fake console on a loopback UDP port: it asks for changes and the
//! console's model, then hears the controller bus's sends and master, and nothing else.
#![cfg(not(target_arch = "wasm32"))]

use soundcraft_net::x32::{Arg, Message, Move, X32Link, decode, encode};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

#[test]
fn the_link_follows_the_controller_bus_of_a_console() {
    let console = UdpSocket::bind("127.0.0.1:0").unwrap();
    console.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let link = X32Link::start(&console.local_addr().unwrap().to_string(), 16, false, None);
    // It asks for changes and for the console's model.
    let mut buf = [0u8; 1024];
    let mut asked = Vec::new();
    let mut client = None;
    while asked.len() < 2 {
        let (n, from) = console.recv_from(&mut buf).unwrap();
        client = Some(from);
        asked.extend(decode(&buf[..n]).into_iter().map(|m| m.addr));
    }
    assert_eq!(asked, ["/xremote", "/info"]);
    let client = client.unwrap();
    let send = |addr: &str, args: Vec<Arg>| {
        console.send_to(&encode(&Message { addr: addr.into(), args }), client).unwrap();
    };
    send("/info", ["V2.07", "osc-server", "X32", "4.06"].iter().map(|s| Arg::Str((*s).into())).collect());
    // The console's own mix (a channel fader, another bus) and the controller bus's moves.
    send("/ch/01/mix/fader", vec![Arg::Float(0.2)]);
    send("/ch/01/mix/05/level", vec![Arg::Float(0.2)]);
    send("/ch/01/mix/16/level", vec![Arg::Float(0.75)]);
    send("/ch/12/mix/16/level", vec![Arg::Float(0.5)]);
    send("/bus/16/mix/fader", vec![Arg::Float(1.0)]);
    let end = Instant::now() + Duration::from_secs(5);
    let mut moves = Vec::new();
    while moves.len() < 3 && Instant::now() < end {
        moves.extend(link.drain());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(moves, [Move::Strip { channel: 1, db: 0.0 }, Move::Strip { channel: 12, db: -10.0 }, Move::Master { db: 10.0 }]);
    let st = link.state();
    assert_eq!(st.console.as_deref(), Some("X32 4.06"));
    assert!(st.last_heard.is_some() && st.error.is_none());
    // A stereo pair (5–6) as the controller from now on: its odd bus, with pans.
    link.set_bus(5, true);
    send("/ch/01/mix/05/level", vec![Arg::Float(0.5)]);
    send("/ch/01/mix/05/pan", vec![Arg::Float(1.0)]);
    let end = Instant::now() + Duration::from_secs(5);
    let mut more = Vec::new();
    while more.len() < 2 && Instant::now() < end {
        more.extend(link.drain());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(more, [Move::Strip { channel: 1, db: -10.0 }, Move::Pan { channel: 1, pan: 1.0 }]);
    link.stop();
}

#[test]
fn an_address_that_does_not_resolve_says_so() {
    let link = X32Link::start("no-such-console.invalid", 16, false, None);
    let end = Instant::now() + Duration::from_secs(10);
    while link.state().error.is_none() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(link.state().error.is_some());
}
