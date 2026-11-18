//! An RTP-MIDI (AppleMIDI) transport: a surface reached over the
//! network, by an Ethernet X-Touch itself or by a computer that shares a
//! USB surface in a network MIDI session. Two UDP ports per unit, the
//! control port and the data port above it.

pub mod pacer;
pub mod payload;
pub mod session;

use std::net::{IpAddr, SocketAddr, UdpSocket as StdUdpSocket};
use std::str::FromStr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::midi::{self, Link, Port};
use pacer::Pacer;
use session::{Config, Event, Session, Socket};

/// An address range: `192.0.2.0/24`, `2001:db8::/32`, or one address.
/// An IPv4-mapped IPv6 range (`::ffff:192.0.2.0/120`) is the IPv4 range
/// it maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: IpAddr,
    bits: u8,
}

impl FromStr for Cidr {
    type Err = String;

    fn from_str(s: &str) -> Result<Cidr, String> {
        let bad = || format!("{s:?} is not an address or address/prefix");
        let (ip, bits) = match s.split_once('/') {
            Some((ip, bits)) => (ip, Some(bits.parse::<u8>().map_err(|_| bad())?)),
            None => (s, None),
        };
        let (net, bits) = match ip.parse::<IpAddr>().map_err(|_| bad())? {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                // The first 96 bits of a mapped address are the mapping.
                Some(v4) => (
                    IpAddr::V4(v4),
                    match bits {
                        Some(b) if b < 96 => return Err(bad()),
                        Some(b) => Some(b - 96),
                        None => None,
                    },
                ),
                None => (IpAddr::V6(v6), bits),
            },
            v4 => (v4, bits),
        };
        let max = if net.is_ipv4() { 32 } else { 128 };
        let bits = bits.unwrap_or(max);
        if bits > max {
            return Err(bad());
        }
        Ok(Cidr { net, bits })
    }
}

impl Cidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        // A prefix longer than the address matches nothing.
        let mask = |len: u32| match len.checked_sub(u32::from(self.bits)) {
            Some(host) if host >= len => Some(0),
            Some(host) => Some(u128::MAX << host),
            None => None,
        };
        match (self.net, ip.to_canonical()) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                mask(32).is_some_and(|m| u128::from(n.to_bits()) & m == u128::from(a.to_bits()) & m)
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                mask(128).is_some_and(|m| n.to_bits() & m == a.to_bits() & m)
            }
            _ => false,
        }
    }
}

/// One unit's session.
#[derive(Debug, Clone)]
pub struct Options {
    /// The address both ports bind to.
    pub bind: IpAddr,
    /// The control port; the data port is the one above it.
    pub port: u16,
    /// The session name the peer sees.
    pub name: String,
    /// Peers whose invitations are accepted.
    pub allow: Vec<Cidr>,
    /// The control port of a peer to invite.
    pub invite: Option<SocketAddr>,
    /// The most octets per second sent to the surface.
    pub max_bytes_per_second: u32,
}

fn random() -> u32 {
    let mut b = [0u8; 4];
    getrandom::fill(&mut b).expect("the operating system's random source");
    u32::from_ne_bytes(b)
}

/// Binds the unit's two ports and runs its session in a task of the
/// current Tokio runtime.
pub fn open(options: Options) -> Result<Port, String> {
    let data_port = options
        .port
        .checked_add(1)
        .ok_or("the data port would be above 65535")?;
    let bind = |port: u16| {
        let s = StdUdpSocket::bind((options.bind, port))
            .map_err(|e| format!("binding {}:{port}: {e}", options.bind))?;
        s.set_nonblocking(true).map_err(|e| e.to_string())?;
        UdpSocket::from_std(s).map_err(|e| e.to_string())
    };
    let control = bind(options.port)?;
    let data = bind(data_port)?;
    let (port, far) = midi::linked_pair();
    tracing::info!(
        name = options.name,
        control = %control.local_addr().map_err(|e| e.to_string())?,
        "RTP-MIDI session listening"
    );
    tokio::spawn(run(options, control, data, far));
    Ok(port)
}

/// What a session event means for the bridge: MIDI goes to it, and a
/// change of the connection, after which the surface is sent its whole
/// state again, replaces what is queued.
fn handle(
    event: Event,
    pacer: &mut Pacer,
    link: &watch::Sender<Link>,
    to_bridge: &UnboundedSender<Vec<u8>>,
) {
    match event {
        Event::Midi(m) => {
            let _ = to_bridge.send(m);
        }
        Event::Up { name, peer } => {
            tracing::info!(%peer, name, "surface connected");
            pacer.clear();
            link.send_modify(|l| {
                l.connected = true;
                l.session += 1;
            });
        }
        Event::Down { reason } => {
            tracing::info!(reason, "surface disconnected");
            pacer.clear();
            link.send_modify(|l| l.connected = false);
        }
        Event::Lost => {
            tracing::debug!("sending the surface's state again");
            pacer.clear();
            link.send_modify(|l| l.resync += 1);
        }
        Event::Missed => link.send_modify(|l| l.missed += 1),
    }
}

async fn run(options: Options, control: UdpSocket, data: UdpSocket, far: midi::Far) {
    let midi::Far {
        to_bridge,
        mut from_bridge,
        link,
    } = far;
    let start = Instant::now();
    let now = || start.elapsed().as_micros() as u64;
    let config = Config {
        name: options.name,
        allow: options.allow,
        invite: options.invite,
    };
    let mut session = Session::new(config, Box::new(random), now());
    let mut pacer = Pacer::new(options.max_bytes_per_second);
    let mut control_buf = vec![0u8; 65536];
    let mut data_buf = vec![0u8; 65536];
    session.tick(now());
    loop {
        while let Some(t) = session.poll_transmit() {
            let socket = match t.socket {
                Socket::Control => &control,
                Socket::Data => &data,
            };
            if let Err(e) = socket.send_to(&t.bytes, t.to).await {
                tracing::debug!(to = %t.to, "sending: {e}");
            }
        }
        while let Some(event) = session.poll_event() {
            handle(event, &mut pacer, &link, &to_bridge);
        }
        let t = now();
        if session.is_up()
            && let Some(packet) = pacer.take(t)
        {
            session.send_midi(&packet, t);
            continue;
        }
        let wake = session
            .deadline()
            .min(pacer.ready_at(t).unwrap_or(u64::MAX))
            .min(t + 60_000_000);
        let sleep = tokio::time::sleep_until(start + Duration::from_micros(wake));
        tokio::select! {
            r = control.recv_from(&mut control_buf) => match r {
                Ok((n, from)) => session.received(Socket::Control, from, &control_buf[..n], now()),
                Err(e) => tracing::debug!("control port: {e}"),
            },
            r = data.recv_from(&mut data_buf) => match r {
                Ok((n, from)) => session.received(Socket::Data, from, &data_buf[..n], now()),
                Err(e) => tracing::debug!("data port: {e}"),
            },
            m = from_bridge.recv() => match m {
                // Without a session nothing is shown; a new one gets
                // the whole state.
                Some(m) => if session.is_up() {
                    pacer.push(m);
                },
                None => break,
            },
            () = sleep => {}
        }
        session.tick(now());
    }
    session.close(now());
    while let Some(t) = session.poll_transmit() {
        let socket = match t.socket {
            Socket::Control => &control,
            Socket::Data => &data,
        };
        let _ = socket.send_to(&t.bytes, t.to).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loss_drops_what_is_queued_and_counts_on_the_link() {
        let (link, rx) = watch::channel(Link::default());
        let (tx, mut to_bridge) = tokio::sync::mpsc::unbounded_channel();
        let mut pacer = Pacer::new(1_000_000);
        pacer.push(vec![0x90, 1, 1]);
        handle(Event::Lost, &mut pacer, &link, &tx);
        assert_eq!(pacer.take(0), None);
        assert_eq!((rx.borrow().resync, rx.borrow().missed), (1, 0));
        pacer.push(vec![0x90, 1, 1]);
        handle(Event::Missed, &mut pacer, &link, &tx);
        assert!(
            pacer.take(0).is_some(),
            "what the peer missed sending keeps the queue"
        );
        assert_eq!((rx.borrow().resync, rx.borrow().missed), (1, 1));
        pacer.push(vec![0x90, 1, 1]);
        handle(Event::Midi(vec![0x90, 2, 2]), &mut pacer, &link, &tx);
        assert_eq!(to_bridge.try_recv().unwrap(), vec![0x90, 2, 2]);
        assert!(
            pacer.take(0).is_some(),
            "MIDI from the peer keeps the queue"
        );
        pacer.push(vec![0x90, 1, 1]);
        let peer = "192.0.2.7:5004".parse().unwrap();
        handle(
            Event::Up {
                name: "x".into(),
                peer,
            },
            &mut pacer,
            &link,
            &tx,
        );
        assert_eq!(pacer.take(0), None);
        assert!(rx.borrow().connected);
        assert_eq!(rx.borrow().session, 1);
        pacer.push(vec![0x90, 1, 1]);
        handle(Event::Down { reason: "test" }, &mut pacer, &link, &tx);
        assert_eq!(pacer.take(0), None);
        assert!(!rx.borrow().connected);
    }

    #[test]
    fn ranges_hold_their_addresses_and_ipv4_mapped_ones() {
        let net: Cidr = "198.51.100.0/24".parse().unwrap();
        assert!(net.contains("198.51.100.4".parse().unwrap()));
        assert!(net.contains("::ffff:198.51.100.4".parse().unwrap()));
        assert!(!net.contains("198.51.101.1".parse().unwrap()));
        assert!(!net.contains("2001:db8::1".parse().unwrap()));
        let one: Cidr = "192.0.2.7".parse().unwrap();
        assert!(one.contains("192.0.2.7".parse().unwrap()));
        assert!(!one.contains("192.0.2.8".parse().unwrap()));
        // Mapped forms, as a dual-stack socket logs them, are the IPv4
        // addresses they map: one address, or a prefix 96 bits longer.
        let mapped: Cidr = "::ffff:192.0.2.7".parse().unwrap();
        assert!(mapped.contains("192.0.2.7".parse().unwrap()));
        assert!(!mapped.contains("192.0.2.8".parse().unwrap()));
        assert!(!mapped.contains("8.8.8.8".parse().unwrap()));
        let mapped: Cidr = "::ffff:198.51.100.0/120".parse().unwrap();
        assert!(mapped.contains("198.51.100.200".parse().unwrap()));
        assert!(!mapped.contains("198.51.101.1".parse().unwrap()));
        assert!(!mapped.contains("8.8.8.8".parse().unwrap()));
        // A prefix that would end inside the mapping is refused.
        assert!("::ffff:198.51.100.0/95".parse::<Cidr>().is_err());
        // Even a range built out of bounds matches nothing, not everything.
        let wide = Cidr {
            net: "192.0.2.0".parse().unwrap(),
            bits: 104,
        };
        assert!(!wide.contains("8.8.8.8".parse().unwrap()));
        let all: Cidr = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains("8.8.8.8".parse().unwrap()));
        let v6: Cidr = "2001:db8::/32".parse().unwrap();
        assert!(v6.contains("2001:db8:1::5".parse().unwrap()));
        assert!(!v6.contains("2001:db9::5".parse().unwrap()));
        for bad in ["192.0.2.0/33", "192.0.2/24", "x", "::/129", "192.0.2.0/"] {
            assert!(bad.parse::<Cidr>().is_err(), "{bad}");
        }
    }

    fn free_ports() -> u16 {
        loop {
            let s = StdUdpSocket::bind("127.0.0.1:0").unwrap();
            let p = s.local_addr().unwrap().port();
            if p < 65535 && StdUdpSocket::bind(("127.0.0.1", p + 1)).is_ok() {
                return p;
            }
        }
    }

    fn options(port: u16, invite: Option<u16>) -> Options {
        Options {
            bind: "127.0.0.1".parse().unwrap(),
            port,
            name: format!("test {port}"),
            allow: vec!["127.0.0.1".parse().unwrap()],
            invite: invite.map(|p| SocketAddr::from(([127, 0, 0, 1], p))),
            max_bytes_per_second: 100_000,
        }
    }

    async fn until(link: &mut watch::Receiver<Link>, f: impl Fn(&Link) -> bool) {
        tokio::time::timeout(Duration::from_secs(5), link.wait_for(|l| f(l)))
            .await
            .expect("in time")
            .unwrap();
    }

    async fn next(port: &mut Port) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(5), port.input.recv())
            .await
            .expect("in time")
            .unwrap()
    }

    #[tokio::test]
    async fn two_ends_over_udp_carry_midi_both_ways_and_a_restart_is_a_new_session() {
        let (a, b) = (free_ports(), free_ports());
        let mut listener = open(options(a, None)).unwrap();
        let mut link = listener.link.clone().unwrap();
        let mut caller = open(options(b, Some(a))).unwrap();
        until(&mut link, |l| l.connected && l.session == 1).await;
        let mut caller_link = caller.link.clone().unwrap();
        until(&mut caller_link, |l| l.connected).await;

        let lcd = crate::mcu::lcd(crate::mcu::Device::Main, 0, &"x".repeat(112));
        listener.output.send(lcd.clone()).unwrap();
        listener.output.send(vec![0x90, 18, 0x7f]).unwrap();
        assert_eq!(next(&mut caller).await, lcd);
        assert_eq!(next(&mut caller).await, vec![0x90, 18, 0x7f]);
        caller.output.send(vec![0x90, 104, 0x7f]).unwrap();
        caller.output.send(vec![0xe0, 0x10, 0x40]).unwrap();
        assert_eq!(next(&mut listener).await, vec![0x90, 104, 0x7f]);
        assert_eq!(next(&mut listener).await, vec![0xe0, 0x10, 0x40]);

        // The caller goes away (says bye), then comes back on the same
        // ports: a new session.
        drop(caller);
        until(&mut link, |l| !l.connected).await;
        // Its task lets go of the ports once it said bye.
        let mut reopened = open(options(b, Some(a)));
        for _ in 0..50 {
            if reopened.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            reopened = open(options(b, Some(a)));
        }
        let caller = reopened.unwrap();
        until(&mut link, |l| l.connected && l.session == 2).await;
        drop(caller);
    }
}
