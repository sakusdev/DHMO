//! Live MIDI input discovery and callback-to-UI event delivery.

use std::{
    sync::mpsc::{self, Receiver},
    time::Instant,
};

use midir::{Ignore, MidiInput, MidiInputConnection};

/// A short channel-voice message received from a physical or virtual MIDI port.
#[derive(Debug, Clone, Copy)]
pub struct LiveMidiMessage {
    pub received_at: Instant,
    bytes: [u8; 3],
    len: u8,
}

impl LiveMidiMessage {
    #[must_use]
    pub(crate) fn from_bytes_at(message: &[u8], received_at: Instant) -> Option<Self> {
        if !(1..=3).contains(&message.len()) {
            return None;
        }
        let mut bytes = [0_u8; 3];
        bytes[..message.len()].copy_from_slice(message);
        Some(Self {
            received_at,
            bytes,
            len: u8::try_from(message.len()).unwrap_or(3),
        })
    }

    #[must_use]
    pub const fn data(self) -> ([u8; 3], u8) {
        (self.bytes, self.len)
    }
}

/// Owns one optional live MIDI input connection.
pub struct MidiInputManager {
    connection: Option<MidiInputConnection<()>>,
    receiver: Option<Receiver<LiveMidiMessage>>,
    connected_name: Option<String>,
}

impl MidiInputManager {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            connection: None,
            receiver: None,
            connected_name: None,
        }
    }

    pub fn ports() -> Result<Vec<String>, String> {
        let input = MidiInput::new("DMO MIDI discovery").map_err(|error| error.to_string())?;
        input
            .ports()
            .iter()
            .map(|port| input.port_name(port).map_err(|error| error.to_string()))
            .collect()
    }

    pub fn connect(&mut self, port_index: usize) -> Result<String, String> {
        let mut input = MidiInput::new("DMO MIDI input").map_err(|error| error.to_string())?;
        input.ignore(Ignore::All);
        let ports = input.ports();
        let port = ports
            .get(port_index)
            .ok_or_else(|| "The selected MIDI input is no longer available".to_owned())?;
        let name = input.port_name(port).map_err(|error| error.to_string())?;
        let (sender, receiver) = mpsc::channel();
        let connection = input
            .connect(
                port,
                "DMO live input",
                move |_timestamp, message, _context| {
                    if let Some(message) = LiveMidiMessage::from_bytes_at(message, Instant::now()) {
                        let _ = sender.send(message);
                    }
                },
                (),
            )
            .map_err(|error| error.to_string())?;
        self.connection = Some(connection);
        self.receiver = Some(receiver);
        self.connected_name = Some(name.clone());
        Ok(name)
    }

    pub fn disconnect(&mut self) {
        self.connection = None;
        self.receiver = None;
        self.connected_name = None;
    }

    #[must_use]
    pub fn connected_name(&self) -> Option<&str> {
        self.connected_name.as_deref()
    }

    pub fn drain(&self) -> Vec<LiveMidiMessage> {
        self.receiver
            .as_ref()
            .map_or_else(Vec::new, |receiver| receiver.try_iter().collect())
    }
}

impl Default for MidiInputManager {
    fn default() -> Self {
        Self::new()
    }
}
