//! The framed protocol shared between the `horde` client and the
//! `horde-runner` session service.
//!
//! Frames are length-prefixed: a 4-byte big-endian length followed by a
//! `bincode`-encoded [`ClientFrame`] or [`ServerFrame`].  The same framing is
//! used on both legs (client↔ssh and the relay↔session socket), so the
//! `serve` relay can shuttle bytes without decoding them.

use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Client → server frames.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClientFrame {
    /// First frame on every connection.  Carries the session parameters (used
    /// only when creating the session) plus the terminal identity and size
    /// (applied on every attach — these can't be read from the non-PTY ssh
    /// environment, so the client supplies them).
    Hello(Hello),
    /// Raw bytes typed by the user, written to the PTY.
    Stdin(Vec<u8>),
    /// Terminal resize.
    Resize { cols: u16, rows: u16 },
}

/// Server → client frames.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ServerFrame {
    /// Raw PTY output to render.
    Output(Vec<u8>),
    /// Sent once the initial screen repaint (on attach) is complete.
    Ready,
    /// The session's claude process exited with this status code.
    Exit(i32),
    /// A fatal error before/while running the session.
    Error(String),
}

/// Metadata a session daemon writes alongside its socket (`<project>.json`),
/// and the shape `horde-runner list` emits (one per live session).  The client
/// reads these to populate the session switcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SessionMeta {
    pub project: String,
    pub extras: Vec<String>,
    pub pid: u32,
    /// Seconds since the Unix epoch when the session started.
    pub started_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Hello {
    pub project: String,
    pub extras: Vec<String>,
    pub prompt_b64: String,
    pub claude_args: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub term: String,
    pub colorterm: String,
    pub lang: String,
    pub lc_all: String,
}

/// Maximum accepted frame length (16 MiB), a guard against a corrupt or
/// hostile length prefix.
const MAX_FRAME: u32 = 16 * 1024 * 1024;

/// Write one length-prefixed frame and flush.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, frame: &T) -> io::Result<()> {
    let body = bincode::serialize(frame).map_err(to_io)?;
    let len = u32::try_from(body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

/// Read one length-prefixed frame.  Returns `Ok(None)` on a clean EOF at a
/// frame boundary (the peer closed the connection).
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<Option<T>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds maximum",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    let frame = bincode::deserialize(&body).map_err(to_io)?;
    Ok(Some(frame))
}

fn to_io(e: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_frames_round_trip() {
        let frames = vec![
            ClientFrame::Hello(Hello {
                project: "api".into(),
                extras: vec!["worker".into()],
                prompt_b64: "aGk=".into(),
                claude_args: vec!["--resume".into()],
                cols: 120,
                rows: 40,
                term: "xterm-256color".into(),
                colorterm: "truecolor".into(),
                lang: "en_US.UTF-8".into(),
                lc_all: String::new(),
            }),
            ClientFrame::Stdin(vec![0x03, b'h', b'i', b'\n']),
            ClientFrame::Resize { cols: 80, rows: 24 },
        ];
        let mut buf = Vec::new();
        for f in &frames {
            write_frame(&mut buf, f).unwrap();
        }
        let mut cur = std::io::Cursor::new(buf);
        for f in &frames {
            let got: ClientFrame = read_frame(&mut cur).unwrap().unwrap();
            assert_eq!(&got, f);
        }
        let end: Option<ClientFrame> = read_frame(&mut cur).unwrap();
        assert_eq!(end, None);
    }

    #[test]
    fn server_frames_round_trip() {
        let frames = vec![
            ServerFrame::Output(vec![1, 2, 3]),
            ServerFrame::Ready,
            ServerFrame::Exit(0),
            ServerFrame::Error("boom".into()),
        ];
        let mut buf = Vec::new();
        for f in &frames {
            write_frame(&mut buf, f).unwrap();
        }
        let mut cur = std::io::Cursor::new(buf);
        for f in &frames {
            let got: ServerFrame = read_frame(&mut cur).unwrap().unwrap();
            assert_eq!(&got, f);
        }
    }
}
