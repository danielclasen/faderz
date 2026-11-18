//! MIDI transports: a surface's USB-MIDI ports through midir (ALSA on
//! Linux), an RTP-MIDI session ([`rtpmidi`]), or an in-process pair whose
//! far end is an [`Emulator`].
//!
//! [`Emulator`]: crate::emulator::Emulator
//! [`rtpmidi`]: crate::rtpmidi

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::watch;

/// What keeps a surface's MIDI ports open.
pub type Open = Option<Box<dyn Send>>;

/// The connection of a surface that comes and goes, as one on the
/// network does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Link {
    pub connected: bool,
    /// Counts the sessions so far: a new one shows nothing yet.
    pub session: u64,
    /// Counts the times the surface may have missed messages and is to
    /// get its whole state again.
    pub resync: u64,
    /// Counts the times messages from the surface went missing, a fader
    /// let go among them perhaps.
    pub missed: u64,
}

/// The bridge's side of a surface: complete MIDI messages in and out.
pub struct Port {
    pub input: UnboundedReceiver<Vec<u8>>,
    pub output: UnboundedSender<Vec<u8>>,
    /// The connection, for a surface that has one of its own; a local
    /// port has none.
    pub link: Option<watch::Receiver<Link>>,
    /// Whatever keeps the ports open while the bridge runs.
    _open: Open,
}

/// What a [`Port`] is made of.
pub struct Parts {
    pub input: UnboundedReceiver<Vec<u8>>,
    pub output: UnboundedSender<Vec<u8>>,
    pub link: Option<watch::Receiver<Link>>,
    pub open: Open,
}

impl Port {
    pub fn into_parts(self) -> Parts {
        Parts {
            input: self.input,
            output: self.output,
            link: self.link,
            open: self._open,
        }
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
            link: None,
            _open: None,
        },
        to_bridge,
        from_bridge,
    )
}

/// The far end of a [`linked_pair`].
pub struct Far {
    pub to_bridge: UnboundedSender<Vec<u8>>,
    pub from_bridge: UnboundedReceiver<Vec<u8>>,
    /// Where the far end reports its connection.
    pub link: watch::Sender<Link>,
}

/// [`pair`] with a [`Link`].
pub fn linked_pair() -> (Port, Far) {
    let (mut port, to_bridge, from_bridge) = pair();
    let (link, rx) = watch::channel(Link::default());
    port.link = Some(rx);
    (
        port,
        Far {
            to_bridge,
            from_bridge,
            link,
        },
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
        link: None,
        _open: Some(Box::new(connection)),
    })
}
