//! MIDI transports: a surface's USB-MIDI ports through midir (ALSA on
//! Linux), or an in-process pair whose far end is an [`Emulator`].
//!
//! [`Emulator`]: crate::emulator::Emulator

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// What keeps a surface's MIDI ports open.
pub type Open = Option<Box<dyn Send>>;

/// The bridge's side of a surface: complete MIDI messages in and out.
pub struct Port {
    pub input: UnboundedReceiver<Vec<u8>>,
    pub output: UnboundedSender<Vec<u8>>,
    /// Whatever keeps the ports open while the bridge runs.
    _open: Open,
}

impl Port {
    /// The input, the output, and what keeps them open.
    pub fn into_parts(self) -> (UnboundedReceiver<Vec<u8>>, UnboundedSender<Vec<u8>>, Open) {
        (self.input, self.output, self._open)
    }
}

/// Two connected ends: the bridge's [`Port`], and the surface's sender
/// and receiver.
pub fn pair() -> (Port, UnboundedSender<Vec<u8>>, UnboundedReceiver<Vec<u8>>) {
    let (to_bridge, input) = unbounded_channel();
    let (output, from_bridge) = unbounded_channel();
    (
        Port {
            input,
            output,
            _open: None,
        },
        to_bridge,
        from_bridge,
    )
}

/// The names of the MIDI input and output ports midir sees.
pub fn list() -> Result<(Vec<String>, Vec<String>), String> {
    let input = midir::MidiInput::new("faderz-bridge").map_err(|e| e.to_string())?;
    let output = midir::MidiOutput::new("faderz-bridge").map_err(|e| e.to_string())?;
    let ins = input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect();
    let outs = output
        .ports()
        .iter()
        .filter_map(|p| output.port_name(p).ok())
        .collect();
    Ok((ins, outs))
}

/// Opens the first input and output ports whose names contain `input`
/// and `output`.
pub fn open(input: &str, output: &str) -> Result<Port, String> {
    let midi_in = midir::MidiInput::new("faderz-bridge").map_err(|e| e.to_string())?;
    let in_port = midi_in
        .ports()
        .into_iter()
        .find(|p| midi_in.port_name(p).is_ok_and(|n| n.contains(input)))
        .ok_or_else(|| format!("no MIDI input port named like {input:?}"))?;
    let midi_out = midir::MidiOutput::new("faderz-bridge").map_err(|e| e.to_string())?;
    let out_port = midi_out
        .ports()
        .into_iter()
        .find(|p| midi_out.port_name(p).is_ok_and(|n| n.contains(output)))
        .ok_or_else(|| format!("no MIDI output port named like {output:?}"))?;
    let (to_bridge, rx) = unbounded_channel();
    let connection = midi_in
        .connect(
            &in_port,
            "faderz-bridge-in",
            move |_, message, _| {
                let _ = to_bridge.send(message.to_vec());
            },
            (),
        )
        .map_err(|e| e.to_string())?;
    let mut out = midi_out
        .connect(&out_port, "faderz-bridge-out")
        .map_err(|e| e.to_string())?;
    let (tx, mut from_bridge) = unbounded_channel::<Vec<u8>>();
    // The output connection is written from a thread of its own.
    std::thread::Builder::new()
        .name(format!("midi-out {output}"))
        .spawn(move || {
            while let Some(message) = from_bridge.blocking_recv() {
                if let Err(e) = out.send(&message) {
                    tracing::warn!("MIDI output: {e}");
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Port {
        input: rx,
        output: tx,
        _open: Some(Box::new(connection)),
    })
}
