//! MIDI output discovery and short-message delivery.

use midir::{MidiOutput, MidiOutputConnection};

/// Owns one optional live MIDI output connection.
pub struct MidiOutputManager {
    connection: Option<MidiOutputConnection>,
    connected_name: Option<String>,
}

impl MidiOutputManager {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            connection: None,
            connected_name: None,
        }
    }

    pub fn ports() -> Result<Vec<String>, String> {
        let output =
            MidiOutput::new("DMO MIDI output discovery").map_err(|error| error.to_string())?;
        output
            .ports()
            .iter()
            .map(|port| output.port_name(port).map_err(|error| error.to_string()))
            .collect()
    }

    pub fn connect(&mut self, port_index: usize) -> Result<String, String> {
        let output = MidiOutput::new("DMO MIDI output").map_err(|error| error.to_string())?;
        let ports = output.ports();
        let port = ports
            .get(port_index)
            .ok_or_else(|| "The selected MIDI output is no longer available".to_owned())?;
        let name = output.port_name(port).map_err(|error| error.to_string())?;
        let connection = output
            .connect(port, "DMO external output")
            .map_err(|error| error.to_string())?;
        self.connection = Some(connection);
        self.connected_name = Some(name.clone());
        Ok(name)
    }

    pub fn disconnect(&mut self) {
        self.connection = None;
        self.connected_name = None;
    }

    #[must_use]
    pub fn connected_name(&self) -> Option<&str> {
        self.connected_name.as_deref()
    }

    pub fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let connection = self
            .connection
            .as_mut()
            .ok_or_else(|| "No MIDI output is connected".to_owned())?;
        connection.send(bytes).map_err(|error| error.to_string())
    }
}

impl Default for MidiOutputManager {
    fn default() -> Self {
        Self::new()
    }
}
