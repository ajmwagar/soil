// SPDX-License-Identifier: MIT OR Apache-2.0
//! One host-accepted HTTP session for the Soil `soil:streams` guest.
//!
//! The listener and socket pumping live in the `soil-serve` binary. This type
//! is deliberately only a bounded byte pipe; HTTP parsing, routing, HTML and
//! SSE framing belong to the Soil program.

use std::sync::mpsc::{Receiver, Sender};

pub struct HttpSession {
    pub protocol: String,
    pub peer: Option<String>,
    pub principal: Option<String>,
    pub media_type: Option<String>,
    inbound: Receiver<Vec<u8>>,
    outbound: Sender<Vec<u8>>,
    pending: Vec<u8>,
    closed: bool,
}

impl HttpSession {
    pub fn new(
        protocol: impl Into<String>,
        peer: Option<String>,
        inbound: Receiver<Vec<u8>>,
        outbound: Sender<Vec<u8>>,
    ) -> Self {
        Self {
            protocol: protocol.into(),
            peer,
            principal: None,
            media_type: Some("text/html; charset=utf-8".into()),
            inbound,
            outbound,
            pending: Vec::new(),
            closed: false,
        }
    }

    pub fn read(&mut self, max: u32) -> Result<Option<Vec<u8>>, &'static str> {
        if max == 0 || max > 1024 * 1024 {
            return Err("over-budget");
        }
        if self.closed {
            return Ok(None);
        }
        if self.pending.is_empty() {
            match self.inbound.recv() {
                Ok(bytes) => self.pending = bytes,
                Err(_) => {
                    self.closed = true;
                    return Ok(None);
                }
            }
        }
        let take = self.pending.len().min(max as usize);
        Ok(Some(self.pending.drain(..take).collect()))
    }

    pub fn write(&mut self, data: &[u8]) -> Result<u64, &'static str> {
        if self.closed {
            return Err("closed");
        }
        if data.len() > 1024 * 1024 {
            return Err("over-budget");
        }
        self.outbound.send(data.to_vec()).map_err(|_| "closed")?;
        Ok(data.len() as u64)
    }

    pub fn close(&mut self) -> Result<(), &'static str> {
        self.closed = true;
        self.pending.clear();
        Ok(())
    }
}
