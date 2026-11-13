//! Persistent LAN lobby with ready states and a host-controlled mesh handoff.
use crate::{GgrsError, Seat, Session, Simulation, connect};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
pub const DEFAULT_PORT: u16 = 7777;
const MAGIC: &[u8] = b"REIGN-LOBBY-3";
const RETRY: Duration = Duration::from_millis(250);
const TIMEOUT: Duration = Duration::from_secs(10);
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub port: u16,
    pub seats: Vec<Seat>,
    pub me: usize,
    /// Stable game-defined character IDs in seat order.
    pub characters: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Matched(Match),
    Failed(String),
}
#[derive(Clone, Debug)]
struct Peer {
    addr: SocketAddr,
    ready: bool,
    character: u8,
    seen: Instant,
    acknowledged: bool,
}
pub struct Lobby {
    socket: UdpSocket,
    port: u16,
    host: Option<SocketAddr>,
    tag: Vec<u8>,
    peers: Vec<Peer>,
    ready: bool,
    character: u8,
    launching: bool,
    prepared: Option<Match>,
    last_send: Option<Instant>,
    heard: Instant,
    done: Option<Status>,
}
impl Lobby {
    pub fn host(port: u16, tag: &str) -> io::Result<Self> {
        Self::open(port, None, tag)
    }
    pub fn join(host: SocketAddr, tag: &str) -> io::Result<Self> {
        Self::open(0, Some(host), tag)
    }
    fn open(port: u16, host: Option<SocketAddr>, tag: &str) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
        socket.set_nonblocking(true)?;
        let port = socket.local_addr()?.port();
        Ok(Self {
            socket,
            port,
            host,
            tag: tag.as_bytes().to_vec(),
            peers: Vec::new(),
            ready: false,
            character: 0,
            launching: false,
            prepared: None,
            last_send: None,
            heard: Instant::now(),
            done: None,
        })
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn is_host(&self) -> bool {
        self.host.is_none()
    }
    pub fn players(&self) -> usize {
        if self.is_host() { self.peers.len() + 1 } else { self.peers.len().max(1) }
    }
    pub fn ready(&self) -> bool {
        self.ready
    }
    pub fn readiness(&self) -> Vec<bool> {
        if self.is_host() {
            std::iter::once(self.ready).chain(self.peers.iter().map(|p| p.ready)).collect()
        } else {
            self.peers.iter().map(|p| p.ready).collect()
        }
    }
    pub fn set_character(&mut self, character: u8) {
        if self.character != character && !self.launching && self.prepared.is_none() {
            self.character = character;
            self.ready = false;
            self.last_send = None;
        }
    }
    pub fn characters(&self) -> Vec<u8> {
        if self.is_host() {
            std::iter::once(self.character).chain(self.peers.iter().map(|p| p.character)).collect()
        } else {
            self.peers.iter().map(|p| p.character).collect()
        }
    }
    pub fn toggle_ready(&mut self) {
        if !self.launching && self.prepared.is_none() {
            self.ready = !self.ready;
            self.last_send = None;
        }
    }
    pub fn launch(&mut self) -> bool {
        if self.is_host()
            && !self.launching
            && self.ready
            && !self.peers.is_empty()
            && self.peers.iter().all(|p| p.ready)
        {
            self.launching = true;
            self.last_send = None;
            true
        } else {
            false
        }
    }
    pub fn poll(&mut self, now: Instant) -> Status {
        if let Some(done) = &self.done {
            return done.clone();
        }
        let mut buf = [0; 65507];
        loop {
            let (len, from) = match self.socket.recv_from(&mut buf) {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(e) => return Status::Failed(e.to_string()),
            };
            if let Some(result) = self.receive(&buf[..len], from, now) {
                self.done = Some(result.clone());
                return result;
            }
        }
        if self.is_host() {
            if self.launching
                && self.peers.iter().any(|p| now.saturating_duration_since(p.seen) >= TIMEOUT)
            {
                return Status::Failed("A player left during launch. Please host again.".into());
            }
            self.peers.retain(|p| now.saturating_duration_since(p.seen) < TIMEOUT);
            if self.launching && self.peers.iter().all(|p| p.acknowledged) && !self.peers.is_empty()
            {
                for p in &self.peers {
                    for _ in 0..5 {
                        self.send(b'G', &[], p.addr);
                    }
                }
                let found = Match {
                    port: self.port,
                    seats: std::iter::once(Seat::Local)
                        .chain(self.peers.iter().map(|p| Seat::Remote(p.addr)))
                        .collect(),
                    me: 0,
                    characters: self.characters(),
                };
                self.done = Some(Status::Matched(found.clone()));
                return Status::Matched(found);
            }
        } else if now.saturating_duration_since(self.heard) >= TIMEOUT {
            return Status::Failed("No answer from the host".into());
        }
        if self.last_send.is_none_or(|t| now.saturating_duration_since(t) >= RETRY) {
            self.last_send = Some(now);
            if let Some(host) = self.host {
                let mut payload = vec![u8::from(self.ready), self.character];
                payload.extend_from_slice(&self.tag);
                self.send(b'H', &payload, host);
                if self.prepared.is_some() {
                    self.send(b'A', &[], host);
                }
            } else {
                let mut roster =
                    format!("0.0.0.0:{},{},{};", self.port, u8::from(self.ready), self.character);
                for p in &self.peers {
                    roster.push_str(&format!("{},{},{};", p.addr, u8::from(p.ready), p.character));
                }
                for (i, p) in self.peers.iter().enumerate() {
                    let payload = format!("{}|{}", i + 1, roster);
                    self.send(if self.launching { b'P' } else { b'S' }, payload.as_bytes(), p.addr);
                }
            }
        }
        Status::Waiting
    }
    fn receive(&mut self, packet: &[u8], from: SocketAddr, now: Instant) -> Option<Status> {
        let Some((&kind, data)) = packet.strip_prefix(MAGIC).and_then(|r| r.split_first()) else {
            // Once a complete roster was acknowledged, a GGRS handshake from
            // the host also commits it if every explicit commit was lost.
            if self.host == Some(from) {
                return self.prepared.clone().map(Status::Matched);
            }
            return None;
        };
        if self.is_host() {
            match kind {
                b'H' if data.get(2..) == Some(self.tag.as_slice()) => {
                    if let Some(p) = self.peers.iter_mut().find(|p| p.addr == from) {
                        p.seen = now;
                        if !self.launching {
                            p.ready = data[0] == 1;
                            p.character = data[1];
                        }
                    } else if !self.launching {
                        self.peers.push(Peer {
                            addr: from,
                            ready: data[0] == 1,
                            character: data[1],
                            seen: now,
                            acknowledged: false,
                        });
                        self.last_send = None;
                    }
                }
                b'H' => self.send(b'R', &[], from),
                b'A' if self.launching => {
                    if let Some(p) = self.peers.iter_mut().find(|p| p.addr == from) {
                        p.acknowledged = true;
                        p.seen = now;
                    }
                }
                b'L' if self.launching && self.peers.iter().any(|p| p.addr == from) => {
                    return Some(Status::Failed(
                        "A player left during launch. Please host again.".into(),
                    ));
                }
                b'L' if !self.launching => {
                    self.peers.retain(|p| p.addr != from);
                    self.last_send = None;
                }
                _ => {}
            }
        } else if self.host == Some(from) {
            self.heard = now;
            match kind {
                b'C' => return Some(Status::Failed("The host closed the lobby".into())),
                b'R' => {
                    return Some(Status::Failed(
                        "The host runs a different version or different game data".into(),
                    ));
                }
                b'G' => return self.prepared.clone().map(Status::Matched),
                b'S' | b'P' => {
                    let text = std::str::from_utf8(data).ok()?;
                    let (me, roster) = text.split_once('|')?;
                    let me: usize = me.parse().ok()?;
                    let peers: Option<Vec<Peer>> = roster
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .enumerate()
                        .map(|(i, s)| {
                            let (info, character) = s.rsplit_once(',')?;
                            let (addr, ready) = info.rsplit_once(',')?;
                            Some(Peer {
                                addr: if i == 0 { from } else { addr.parse().ok()? },
                                ready: ready == "1",
                                character: character.parse().ok()?,
                                seen: now,
                                acknowledged: false,
                            })
                        })
                        .collect();
                    let peers = peers?;
                    if me == 0 || me >= peers.len() {
                        return None;
                    }
                    self.peers = peers;
                    if kind == b'P' {
                        self.prepared = Some(Match {
                            port: self.port,
                            me,
                            characters: self.characters(),
                            seats: self
                                .peers
                                .iter()
                                .enumerate()
                                .map(
                                    |(i, p)| {
                                        if i == me { Seat::Local } else { Seat::Remote(p.addr) }
                                    },
                                )
                                .collect(),
                        });
                        self.send(b'A', &[], from);
                    }
                }
                _ => {}
            }
        }
        None
    }
    fn send(&self, kind: u8, data: &[u8], to: SocketAddr) {
        let mut packet = MAGIC.to_vec();
        packet.push(kind);
        packet.extend_from_slice(data);
        let _ = self.socket.send_to(&packet, to);
    }
    pub fn start<S: Simulation>(self, found: &Match) -> Result<Session<S>, GgrsError> {
        drop(self);
        connect(found.port, &found.seats)
    }
}
impl Drop for Lobby {
    fn drop(&mut self) {
        if self.done.as_ref().is_some_and(|s| matches!(s, Status::Matched(_))) {
            return;
        }
        if let Some(host) = self.host {
            self.send(b'L', &[], host);
        } else {
            for p in &self.peers {
                self.send(b'C', &[], p.addr);
            }
        }
    }
}
/// Reads an address typed by a player: an IP or host name, with or without
/// a port (the default port if not).
pub fn parse_address(text: &str) -> Option<SocketAddr> {
    use std::net::ToSocketAddrs;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let with_port =
        if text.contains(':') { text.to_string() } else { format!("{text}:{DEFAULT_PORT}") };
    // Prefer IPv4: the lobby and the session listen on IPv4.
    let addrs: Vec<SocketAddr> = with_port.to_socket_addrs().ok()?.collect();
    addrs.iter().find(|a| a.is_ipv4()).or(addrs.first()).copied()
}

/// This machine's address on its local network, to tell a joiner. Asks the
/// OS which interface it would route to the internet through; nothing is
/// sent.
pub fn local_ip() -> Option<std::net::IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).ok()?;
    Some(socket.local_addr().ok()?.ip()).filter(|ip| !ip.is_unspecified())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn address(lobby: &Lobby) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, lobby.port()))
    }
    fn pump(host: &mut Lobby, guests: &mut [Lobby]) {
        for _ in 0..5 {
            for guest in guests.iter_mut() {
                assert_eq!(guest.poll(Instant::now()), Status::Waiting);
            }
            assert_eq!(host.poll(Instant::now()), Status::Waiting);
            std::thread::sleep(Duration::from_millis(3));
        }
    }
    #[test]
    fn party_waits_for_readiness_and_explicit_host_launch() {
        let mut host = Lobby::host(0, "test").unwrap();
        let mut guests: Vec<_> =
            (0..3).map(|_| Lobby::join(address(&host), "test").unwrap()).collect();
        host.toggle_ready();
        pump(&mut host, &mut guests);
        assert_eq!(host.players(), 4);
        assert!(guests.iter().all(|g| g.players() == 4));
        assert!(!host.launch());
        assert!(guests.iter_mut().all(|g| !g.launch()));
        for (i, g) in guests.iter_mut().enumerate() {
            g.set_character(((i + 1) % 2) as u8);
            g.toggle_ready();
        }
        pump(&mut host, &mut guests);
        assert!(host.readiness().iter().all(|r| *r));
        assert_eq!(host.characters(), vec![0, 1, 0, 1]);
        assert!(host.launch());
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut matches = vec![None; 4];
        while matches.iter().any(Option::is_none) {
            assert!(Instant::now() < deadline, "party failed to launch");
            for (i, lobby) in std::iter::once(&mut host).chain(guests.iter_mut()).enumerate() {
                match lobby.poll(Instant::now()) {
                    Status::Matched(found) => matches[i] = Some(found),
                    Status::Failed(why) => panic!("{why}"),
                    Status::Waiting => {}
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        for (i, found) in matches.iter().enumerate() {
            let found = found.as_ref().unwrap();
            assert_eq!(found.seats.len(), 4);
            assert_eq!(found.me, i);
            assert_eq!(found.characters, vec![0, 1, 0, 1]);
            for (j, seat) in found.seats.iter().enumerate() {
                assert_eq!(
                    *seat,
                    if i == j {
                        Seat::Local
                    } else if j == 0 {
                        Seat::Remote(address(&host))
                    } else {
                        Seat::Remote(address(&guests[j - 1]))
                    }
                );
            }
        }
    }
    #[test]
    fn changing_character_revokes_ready_and_updates_the_host_manifest() {
        let mut host = Lobby::host(0, "test").unwrap();
        let mut guests = vec![Lobby::join(address(&host), "test").unwrap()];
        host.toggle_ready();
        guests[0].toggle_ready();
        pump(&mut host, &mut guests);
        guests[0].set_character(1);
        assert!(!guests[0].ready());
        pump(&mut host, &mut guests);
        assert_eq!(host.characters(), vec![0, 1]);
        assert!(!host.launch());
        guests[0].toggle_ready();
        pump(&mut host, &mut guests);
        assert!(host.launch());
    }

    #[test]
    fn leaving_and_host_closure_update_the_lobby() {
        let mut host = Lobby::host(0, "test").unwrap();
        let mut guests = vec![
            Lobby::join(address(&host), "test").unwrap(),
            Lobby::join(address(&host), "test").unwrap(),
        ];
        pump(&mut host, &mut guests);
        drop(guests.pop());
        pump(&mut host, &mut guests);
        assert_eq!(host.players(), 2);
        assert_eq!(guests[0].players(), 2);
        drop(host);
        // Closing the host sends a datagram; its delivery is asynchronous.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match guests[0].poll(Instant::now()) {
                Status::Failed(_) => break,
                Status::Waiting => {
                    assert!(Instant::now() < deadline, "host closure was not delivered");
                    std::thread::sleep(Duration::from_millis(2));
                }
                Status::Matched(_) => panic!("closed lobby cannot launch"),
            }
        }
    }
    #[test]
    fn expired_peer_is_removed_before_launch_and_fails_during_launch() {
        let mut host = Lobby::host(0, "test").unwrap();
        let mut guests = vec![Lobby::join(address(&host), "test").unwrap()];
        pump(&mut host, &mut guests);
        assert_eq!(host.poll(Instant::now() + TIMEOUT), Status::Waiting);
        assert_eq!(host.players(), 1);
        pump(&mut host, &mut guests);
        host.toggle_ready();
        guests[0].toggle_ready();
        pump(&mut host, &mut guests);
        assert!(host.launch());
        assert!(matches!(host.poll(Instant::now() + TIMEOUT), Status::Failed(_)));
    }
    #[test]
    fn incompatible_or_unrelated_packets_cannot_join_or_start() {
        let mut host = Lobby::host(0, "v1").unwrap();
        let mut guest = Lobby::join(address(&host), "v2").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            assert!(Instant::now() < deadline, "incompatible version was not rejected");
            assert_eq!(host.poll(Instant::now()), Status::Waiting);
            if matches!(guest.poll(Instant::now()), Status::Failed(_)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut guest = Lobby::join(address(&host), "v1").unwrap();
        assert_eq!(guest.receive(b"random bytes", address(&host), Instant::now()), None);
        assert_eq!(guest.players(), 1);
    }
    #[test]
    fn addresses_take_the_default_port() {
        assert_eq!(
            parse_address("127.0.0.1"),
            Some(SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT)))
        );
        assert_eq!(parse_address(" 10.0.0.2:9000 "), "10.0.0.2:9000".parse().ok());
        assert_eq!(parse_address(""), None);
    }
}
