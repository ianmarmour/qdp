//! Decoding in a separate process (Section 11.5). The client's decoder
//! parses video from the host, which is untrusted input, so it can run in a
//! child process of its own: a bug in the decoder that a crafted stream
//! exploits then reaches a process that holds none of the client's session
//! keys, clipboard or input, rather than the client itself.
//!
//! The parent sends each temporal unit to the child on its standard input
//! and reads the decoded picture back from its standard output. The child
//! is untrusted too once it has decoded a stream, so the parent checks
//! every reply before it allocates anything for it, and replaces a child
//! that misbehaves or dies with a new one.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use qdp::message::MAX_FRAME_PAYLOAD_LENGTH;
use tracing::{debug, warn};

use crate::{CodecError, CodecPreference, VideoDecoder, Yuv420Frame, open_decoder};

/// The bytes that open the parent's first message, naming the protocol and
/// its version, so that a program started by mistake fails at once.
const HELLO: [u8; 5] = *b"QDPD\x01";

/// The longest side of a picture QDP allows (Section 6.3), and so the
/// longest the parent accepts from the child.
const MAX_SIDE: u32 = 16384;

/// The longest message, a description or an error, either side sends.
const MAX_MESSAGE: usize = 1024;

/// The status byte of a reply that succeeded.
const SUCCEEDED: u8 = 0;

/// The status byte of a reply that failed, followed by why.
const FAILED: u8 = 1;

/// How a [`ProcessDecoder`] describes itself after the child's decoder.
const IN_A_PROCESS: &str = ", in a separate process";

/// How to start a decoder process: a program, run with these arguments,
/// that calls [`serve_decoder_on_stdio`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecoderCommand {
    /// The program, such as the client's own executable.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
}

/// A decoder in a child process, started with a [`DecoderCommand`].
#[derive(Debug)]
pub struct ProcessDecoder {
    /// How to start the child.
    command: DecoderCommand,
    /// The child decoder's software thread count, or 0 for automatic.
    threads: u32,
    /// Which decoder the child opens.
    preference: CodecPreference,
    /// The running child, until it fails.
    process: Option<DecoderProcess>,
    /// The child decoder's description, for logs.
    description: String,
}

impl ProcessDecoder {
    /// Starts a child with `command` that decodes as `preference` asks, with
    /// `threads` software decoder threads, or 0 to choose automatically.
    pub fn spawn(
        command: DecoderCommand,
        threads: u32,
        preference: CodecPreference,
    ) -> Result<Self, CodecError> {
        let (process, description) = DecoderProcess::start(&command, threads, preference)?;
        Ok(Self {
            command,
            threads,
            preference,
            process: Some(process),
            description: format!("{description}{IN_A_PROCESS}"),
        })
    }
}

impl VideoDecoder for ProcessDecoder {
    fn decode(&mut self, temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
        let process = match &mut self.process {
            Some(process) => process,
            None => {
                // The last child failed: a new one starts with the next
                // frame, which the client makes a key frame.
                let (process, _) =
                    DecoderProcess::start(&self.command, self.threads, self.preference)?;
                self.process.insert(process)
            }
        };
        match process.decode(temporal_unit) {
            Ok(result) => result,
            Err(error) => {
                warn!(%error, "the decoder process failed; it is replaced");
                self.process = None;
                Err(CodecError::DecoderProcess(error.to_string()))
            }
        }
    }

    fn description(&self) -> &str {
        &self.description
    }
}

/// A running decoder process and the pipes to it.
#[derive(Debug)]
struct DecoderProcess {
    /// The child.
    child: Child,
    /// Its standard input, which takes temporal units.
    input: ChildStdin,
    /// Its standard output, which gives pictures.
    output: BufReader<ChildStdout>,
}

impl DecoderProcess {
    /// Starts a child with `command` and opens its decoder, returning the
    /// process and the decoder's description.
    fn start(
        command: &DecoderCommand,
        threads: u32,
        preference: CodecPreference,
    ) -> Result<(Self, String), CodecError> {
        let failed = |error: io::Error| {
            CodecError::DecoderProcess(format!(
                "cannot start {}: {error}",
                command.program.display()
            ))
        };
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(failed)?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(CodecError::DecoderProcess(
                "the decoder process has no pipes".to_owned(),
            ));
        };
        enlarge_pipe(&output);
        enlarge_pipe(&input);
        let mut process = Self {
            child,
            input,
            output: BufReader::new(output),
        };
        let description = process
            .open(threads, preference)
            .map_err(|error| CodecError::DecoderProcess(error.to_string()))??;
        debug!(pid = process.child.id(), "decoder process started");
        Ok((process, description))
    }

    /// Asks the child to open its decoder, returning its description or
    /// why it could not.
    fn open(
        &mut self,
        threads: u32,
        preference: CodecPreference,
    ) -> io::Result<Result<String, CodecError>> {
        let mut hello = HELLO.to_vec();
        hello.push(preference_byte(preference));
        hello.extend_from_slice(&threads.to_le_bytes());
        self.input.write_all(&hello)?;
        let status = read_status(&mut self.output)?;
        let message = read_message(&mut self.output)?;
        Ok(match status {
            SUCCEEDED => Ok(message),
            _ => Err(CodecError::DecoderProcess(message)),
        })
    }

    /// Sends a temporal unit to the child and reads its picture, or the
    /// decoder's error. An I/O error means the child failed.
    fn decode(&mut self, temporal_unit: &[u8]) -> io::Result<Result<Yuv420Frame, CodecError>> {
        let length = u32::try_from(temporal_unit.len())
            .ok()
            .filter(|&length| length as usize <= MAX_FRAME_PAYLOAD_LENGTH)
            .ok_or_else(|| invalid("the temporal unit is too large"))?;
        self.input.write_all(&length.to_le_bytes())?;
        self.input.write_all(temporal_unit)?;
        if read_status(&mut self.output)? == FAILED {
            return Ok(Err(CodecError::DecoderProcess(read_message(
                &mut self.output,
            )?)));
        }
        let width = read_side(&mut self.output)?;
        let height = read_side(&mut self.output)?;
        let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
        let y = read_plane(&mut self.output, width * height)?;
        let u = read_plane(&mut self.output, chroma_width * chroma_height)?;
        let v = read_plane(&mut self.output, chroma_width * chroma_height)?;
        Yuv420Frame::from_planes(width, height, y, u, v)
            .map(Ok)
            .ok_or_else(|| invalid("the picture's planes do not match its size"))
    }
}

impl Drop for DecoderProcess {
    fn drop(&mut self) {
        // The child would end on its own once its input closes; killing it
        // also ends one that is stuck.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs a decoder process on this process's standard input and output:
/// what the program a [`DecoderCommand`] names calls, and nothing else
/// may write to standard output meanwhile.
pub fn serve_decoder_on_stdio() -> io::Result<()> {
    let output = raw_stdout()?;
    serve_decoder(io::stdin().lock(), output)
}

/// Opens the decoder a parent asks for on `input`, then decodes each
/// temporal unit that arrives on `input`, writing the pictures to
/// `output`, until `input` ends.
pub fn serve_decoder(mut input: impl Read, mut output: impl Write) -> io::Result<()> {
    let mut hello = [0; HELLO.len()];
    input.read_exact(&mut hello)?;
    if hello != HELLO {
        return Err(invalid("this is not a QDP decoder request"));
    }
    let preference = preference_from_byte(read_u8(&mut input)?)?;
    let threads = read_u32(&mut input)?;
    let mut decoder = match open_decoder(threads, preference) {
        Ok(decoder) => {
            write_reply(&mut output, SUCCEEDED, decoder.description())?;
            decoder
        }
        Err(error) => return write_reply(&mut output, FAILED, &error.to_string()),
    };
    let mut temporal_unit = Vec::new();
    loop {
        let length = match read_u32(&mut input) {
            Ok(length) => length as usize,
            // The parent closed the pipe: the session is over.
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        if length > MAX_FRAME_PAYLOAD_LENGTH {
            return Err(invalid("the temporal unit is too large"));
        }
        temporal_unit.resize(length, 0);
        input.read_exact(&mut temporal_unit)?;
        match decoder.decode(&temporal_unit) {
            Ok(picture) => {
                output.write_all(&[SUCCEEDED])?;
                for side in [picture.width(), picture.height()] {
                    output.write_all(&(side as u32).to_le_bytes())?;
                }
                output.write_all(picture.y())?;
                output.write_all(picture.u())?;
                output.write_all(picture.v())?;
            }
            Err(error) => {
                output.write_all(&[FAILED])?;
                write_message(&mut output, &error.to_string())?;
            }
        }
        output.flush()?;
    }
}

/// Returns this process's standard output as a file, written without the
/// line buffering of [`io::Stdout`], which would split pictures at every
/// newline byte.
fn raw_stdout() -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        Ok(File::from(io::stdout().as_fd().try_clone_to_owned()?))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        Ok(File::from(io::stdout().as_handle().try_clone_to_owned()?))
    }
}

/// The pipe buffer asked for on Linux, which takes a picture in a few
/// writes rather than many 64 KiB ones, each waiting for the reader.
#[cfg(target_os = "linux")]
const PIPE_SIZE: libc::c_int = 1 << 20;

/// Enlarges the buffer of the pipe `end` belongs to, where the platform
/// allows; a pipe that keeps its size only takes more writes.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn enlarge_pipe(end: &impl std::os::fd::AsRawFd) {
    // SAFETY: F_SETPIPE_SZ takes an open pipe descriptor and an int, and
    // fails harmlessly if the size is not allowed.
    unsafe { libc::fcntl(end.as_raw_fd(), libc::F_SETPIPE_SZ, PIPE_SIZE) };
}

/// Leaves pipe buffers at their size where they cannot be enlarged.
#[cfg(not(target_os = "linux"))]
fn enlarge_pipe<T>(_end: &T) {}

/// Writes a reply's status and message.
fn write_reply(output: &mut impl Write, status: u8, message: &str) -> io::Result<()> {
    output.write_all(&[status])?;
    write_message(output, message)?;
    output.flush()
}

/// Writes a message, cut to [`MAX_MESSAGE`] bytes, after its length.
fn write_message(output: &mut impl Write, message: &str) -> io::Result<()> {
    let mut end = message.len().min(MAX_MESSAGE);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    output.write_all(&(end as u16).to_le_bytes())?;
    output.write_all(&message.as_bytes()[..end])
}

/// Reads a message written by [`write_message`].
fn read_message(input: &mut impl Read) -> io::Result<String> {
    let mut length = [0; 2];
    input.read_exact(&mut length)?;
    let length = usize::from(u16::from_le_bytes(length));
    if length > MAX_MESSAGE {
        return Err(invalid("the message is too long"));
    }
    let mut message = vec![0; length];
    input.read_exact(&mut message)?;
    Ok(String::from_utf8_lossy(&message).into_owned())
}

/// Reads a reply's status byte.
fn read_status(input: &mut impl Read) -> io::Result<u8> {
    match read_u8(input)? {
        status @ (SUCCEEDED | FAILED) => Ok(status),
        _ => Err(invalid("the reply has an unknown status")),
    }
}

/// Reads a picture's width or height, which must be within QDP's limits.
fn read_side(input: &mut impl Read) -> io::Result<usize> {
    let side = read_u32(input)?;
    if side == 0 || side > MAX_SIDE {
        return Err(invalid("the picture's size is out of range"));
    }
    Ok(side as usize)
}

/// Reads a plane of `length` bytes.
fn read_plane(input: &mut impl Read, length: usize) -> io::Result<Vec<u8>> {
    let mut plane = Vec::with_capacity(length);
    input.take(length as u64).read_to_end(&mut plane)?;
    if plane.len() != length {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    Ok(plane)
}

/// Reads one byte.
fn read_u8(input: &mut impl Read) -> io::Result<u8> {
    let mut byte = [0];
    input.read_exact(&mut byte)?;
    Ok(byte[0])
}

/// Reads a little-endian 32-bit integer.
fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

/// Returns the byte that carries `preference` to the child.
fn preference_byte(preference: CodecPreference) -> u8 {
    match preference {
        CodecPreference::Auto => 0,
        CodecPreference::Hardware => 1,
        CodecPreference::Software => 2,
    }
}

/// Returns the preference a byte from the parent carries.
fn preference_from_byte(byte: u8) -> io::Result<CodecPreference> {
    match byte {
        0 => Ok(CodecPreference::Auto),
        1 => Ok(CodecPreference::Hardware),
        2 => Ok(CodecPreference::Software),
        _ => Err(invalid("unknown decoder preference")),
    }
}

/// Returns an error for data that breaks the protocol.
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Av1Encoder, EncoderSettings, FASTEST_SPEED, KeyFrameRequest};

    /// Returns one temporal unit: a key frame of a `side × side` picture.
    fn key_frame(side: usize) -> Vec<u8> {
        let mut encoder = Av1Encoder::new(EncoderSettings {
            width: side,
            height: side,
            frame_rate: 60,
            speed: FASTEST_SPEED,
            quantizer: 100,
            threads: 1,
        })
        .unwrap();
        let mut encoded = encoder
            .encode(&Yuv420Frame::black(side, side), KeyFrameRequest::Force)
            .unwrap();
        encoded.extend(encoder.flush().unwrap());
        encoded.remove(0).temporal_unit
    }

    /// Returns the bytes a parent sends: the hello, then each temporal unit.
    fn request(temporal_units: &[&[u8]]) -> Vec<u8> {
        let mut bytes = HELLO.to_vec();
        bytes.push(preference_byte(CodecPreference::Software));
        bytes.extend_from_slice(&1u32.to_le_bytes());
        for temporal_unit in temporal_units {
            bytes.extend_from_slice(&(temporal_unit.len() as u32).to_le_bytes());
            bytes.extend_from_slice(temporal_unit);
        }
        bytes
    }

    /// The child opens its decoder, returns the picture of a temporal unit,
    /// reports a decoder error for garbage, and ends with its input.
    #[test]
    fn serves_pictures() {
        let side = 64;
        let frame = key_frame(side);
        let mut output = Vec::new();
        serve_decoder(&request(&[&frame, b"garbage"])[..], &mut output).unwrap();
        let mut reply = &output[..];
        assert_eq!(read_status(&mut reply).unwrap(), SUCCEEDED);
        assert!(read_message(&mut reply).unwrap().starts_with("rav1d"));
        assert_eq!(read_status(&mut reply).unwrap(), SUCCEEDED);
        assert_eq!(read_side(&mut reply).unwrap(), side);
        assert_eq!(read_side(&mut reply).unwrap(), side);
        let planes = side * side * 3 / 2;
        reply = &reply[planes..];
        assert_eq!(read_status(&mut reply).unwrap(), FAILED);
        assert!(!read_message(&mut reply).unwrap().is_empty());
        assert!(reply.is_empty());
    }

    /// A child that is not a QDP decoder, or that sends a picture larger
    /// than QDP allows, is refused before anything is allocated for it.
    #[test]
    fn refuses_bad_peers() {
        assert!(serve_decoder(&b"HTTP/1.1"[..], Vec::new()).is_err());
        let mut huge = vec![SUCCEEDED];
        huge.extend_from_slice(&(MAX_SIDE + 1).to_le_bytes());
        let mut reply = &huge[..];
        assert_eq!(read_status(&mut reply).unwrap(), SUCCEEDED);
        assert!(read_side(&mut reply).is_err());
        assert!(read_status(&mut &[7][..]).is_err());
        assert!(read_plane(&mut &[1, 2][..], 3).is_err());
    }
}
