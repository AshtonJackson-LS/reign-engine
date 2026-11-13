//! Two peers playing a toy game over real UDP, for trying the netcode
//! between two terminals or two machines on a LAN:
//!
//! ```sh
//! cargo run -p reign_net --example udp_peer -- 0 7000 127.0.0.1:7001
//! cargo run -p reign_net --example udp_peer -- 1 7001 127.0.0.1:7000
//! ```
//!
//! Arguments: this peer's player number (0 or 1), the UDP port to listen
//! on, and the other peer's address. Each peer presses scripted buttons,
//! and once a second prints its tick, ping, frames ahead and rollbacks.
//! A desync stops both. Each runs for 20 seconds.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use reign_core::{FixedClock, InputFrame, Rng, TICK_HZ, checksum};
use reign_net::{GgrsError, GgrsEvent, Seat, SessionState, Simulation};

/// Each player's position follows their held buttons; `history` folds in
/// every input so any disagreement shows in the checksum.
#[derive(Clone, Hash)]
struct Toy {
    pos: [i32; 2],
    history: u64,
}

impl Simulation for Toy {
    fn step(&mut self, inputs: &[InputFrame]) {
        for (p, input) in inputs.iter().enumerate() {
            self.pos[p] += input.axis_x();
            self.history = self.history.rotate_left(7) ^ input.bits() as u64;
        }
    }

    fn checksum(&self) -> u64 {
        checksum(self)
    }
}

fn main() {
    let usage = "usage: udp_peer <player 0|1> <listen port> <peer addr>";
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [me, port, peer] = args.as_slice() else {
        eprintln!("{usage}");
        std::process::exit(2);
    };
    let me: usize = me.parse().ok().filter(|p| *p < 2).expect(usage);
    let port: u16 = port.parse().expect(usage);
    let peer: SocketAddr = peer.parse().expect(usage);
    let mut seats = [Seat::Remote(peer); 2];
    seats[me] = Seat::Local;
    let mut session = reign_net::connect::<Toy>(port, &seats).expect("start the session");

    let mut toy = Toy { pos: [0; 2], history: 0 };
    let mut clock = FixedClock::new(TICK_HZ);
    let mut rng = Rng::new(me as u64 + 1);
    let mut held = InputFrame::default();
    let (start, mut last, mut report) = (Instant::now(), Instant::now(), Instant::now());
    let (mut skip, mut rollbacks) = (0, 0);
    println!("player {me} on port {port}, waiting for {peer}");
    while start.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(1));
        let now = Instant::now();
        let ticks = clock.advance(now - last);
        last = now;
        session.poll_remote_clients();
        for event in session.events() {
            match event {
                GgrsEvent::WaitRecommendation { skip_frames } => skip += skip_frames,
                GgrsEvent::DesyncDetected { frame, .. } => panic!("desync at tick {frame}"),
                GgrsEvent::Disconnected { .. } => return println!("the other peer left"),
                GgrsEvent::Synchronized { .. } => println!("connected"),
                _ => {}
            }
        }
        if session.current_state() != SessionState::Running {
            continue;
        }
        for _ in 0..ticks {
            // Ahead of the other peer: let it catch up.
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if rng.range(0, 30) == 0 {
                held = InputFrame::from_bits(rng.range(0, 0xFF) as u16);
            }
            session.add_local_input(me, held.bits()).expect("local input");
            match session.advance_frame() {
                Ok(requests) => rollbacks += (reign_net::fulfil(&mut toy, requests) > 1) as u32,
                Err(GgrsError::PredictionThreshold) => break,
                Err(e) => panic!("{e}"),
            }
        }
        if now - report >= Duration::from_secs(1) {
            report = now;
            let ping = session.network_stats(1 - me).map_or(0, |s| s.ping);
            println!(
                "tick {:>5}  ping {ping:>3} ms  ahead {:>2}  rollbacks {rollbacks}",
                session.current_frame(),
                session.frames_ahead(),
            );
        }
    }
}
