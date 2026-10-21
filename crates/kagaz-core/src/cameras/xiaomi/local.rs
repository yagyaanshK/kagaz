//! Finding the account's Xiaomi cameras on this network. Every Mi Home
//! device answers the miIO "hello" on UDP 54321 with its device ID, the
//! same number the cloud lists as `did`, so a camera is paired with its
//! cloud entry by that alone and its current address taken from the answer.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const PORT: u16 = 54321;

/// The miIO hello: magic 0x2131, length 32, the rest all ones.
fn hello() -> [u8; 32] {
    let mut h = [0xffu8; 32];
    h[..4].copy_from_slice(&[0x21, 0x31, 0x00, 0x20]);
    h
}

/// The device ID in a hello answer, if `packet` is one.
pub fn device_id(packet: &[u8]) -> Option<String> {
    if packet.len() < 32 || packet[..2] != [0x21, 0x31] {
        return None;
    }
    let id = u32::from_be_bytes(packet[8..12].try_into().ok()?);
    (id != 0 && id != u32::MAX).then(|| id.to_string())
}

/// Ask the whole network, and `also` directly (the addresses the cloud
/// last saw), and collect device ID → address for `timeout`.
pub fn find(also: &[Ipv4Addr], timeout: Duration) -> HashMap<String, Ipv4Addr> {
    let mut out = HashMap::new();
    let Ok(sock) = UdpSocket::bind(("0.0.0.0", 0)) else { return out };
    let _ = sock.set_broadcast(true);
    let packet = hello();
    let _ = sock.send_to(&packet, (Ipv4Addr::BROADCAST, PORT));
    for ip in also {
        let _ = sock.send_to(&packet, (*ip, PORT));
    }
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 256];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || sock.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match sock.recv_from(&mut buf) {
            Ok((n, SocketAddr::V4(from))) => {
                if let Some(id) = device_id(&buf[..n]) {
                    out.insert(id, *from.ip());
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hello_answer_carries_the_device_id() {
        let mut answer = [0u8; 32];
        answer[..4].copy_from_slice(&[0x21, 0x31, 0x00, 0x20]);
        answer[8..12].copy_from_slice(&123456789u32.to_be_bytes());
        assert_eq!(device_id(&answer).as_deref(), Some("123456789"));
        assert_eq!(device_id(&hello()), None); // our own question
        assert_eq!(device_id(&answer[..20]), None);
    }
}
