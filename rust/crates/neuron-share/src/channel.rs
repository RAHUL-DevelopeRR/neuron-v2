//! The computer's end of one encrypted channel: its keys and frame counters. Counters start at 0
//! in each direction and go up by one per frame; a receiver takes only the next one, so the relay
//! cannot replay, reorder or drop a frame without the channel noticing.

use crate::crypto::{open_frame, seal_frame, ChannelKeys, Direction, E2eError};

/// What opening a received frame gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    Frame(String),
    /// Already opened (the relay replayed it after a resume).
    Duplicate,
    /// Frames before it are missing.
    Gap {
        expected: u64,
    },
}

#[derive(Debug)]
pub struct ChannelCrypto {
    ch: String,
    keys: ChannelKeys,
    next_send: u64,
    /// The next counter this end takes.
    next_receive: u64,
}

impl ChannelCrypto {
    /// The computer's side: it seals `h2p` and opens `p2h`.
    #[must_use]
    pub fn host(ch: &str, keys: ChannelKeys) -> Self {
        Self {
            ch: ch.to_string(),
            keys,
            next_send: 0,
            next_receive: 0,
        }
    }

    /// Seals the next frame: its counter and ciphertext.
    pub fn seal(&mut self, text: &str) -> (u64, String) {
        let n = self.next_send;
        self.next_send += 1;
        (
            n,
            seal_frame(&self.keys.h2p, &self.ch, Direction::H2p, n, text),
        )
    }

    /// Opens a received frame. An error means it was not sealed by the paired phone.
    pub fn open(&mut self, n: u64, c: &str) -> Result<Opened, E2eError> {
        if n < self.next_receive {
            return Ok(Opened::Duplicate);
        }
        if n != self.next_receive {
            return Ok(Opened::Gap {
                expected: self.next_receive,
            });
        }
        let text = open_frame(&self.keys.p2h, &self.ch, Direction::P2h, n, c)?;
        self.next_receive += 1;
        Ok(Opened::Frame(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_both_ways_and_drops_replays() {
        let keys = ChannelKeys {
            p2h: [1; 32],
            h2p: [2; 32],
        };
        let mut channel = ChannelCrypto::host("channel-1", keys.clone());
        let (n, c) = channel.seal("one");
        assert_eq!(n, 0);
        assert_eq!(
            crate::crypto::open_frame(&keys.h2p, "channel-1", Direction::H2p, 0, &c).unwrap(),
            "one"
        );
        assert_eq!(channel.seal("two").0, 1);
        let from_phone = |n| seal_frame(&keys.p2h, "channel-1", Direction::P2h, n, "hi");
        assert_eq!(
            channel.open(1, &from_phone(1)).unwrap(),
            Opened::Gap { expected: 0 }
        );
        assert_eq!(
            channel.open(0, &from_phone(0)).unwrap(),
            Opened::Frame("hi".into())
        );
        assert_eq!(channel.open(0, &from_phone(0)).unwrap(), Opened::Duplicate);
        assert!(
            channel.open(1, &from_phone(2)).is_err(),
            "sealed with another counter"
        );
    }
}
