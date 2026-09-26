use crate::{
    read_header, unique_destination, write_header, Header, PayloadKind, ProtocolError, ACK_ERROR,
    ACK_OK, DISCOVERY_PORT, DISCOVERY_QUERY, DISCOVERY_RESPONSE_PREFIX, MAX_TEXT_BYTES,
};
use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{IpAddr, Shutdown, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

pub type NetworkResult<T> = Result<T, NetworkError>;

#[derive(Debug)]
pub enum NetworkError {
    Io(io::Error),
    Protocol(ProtocolError),
    InvalidUtf8(std::string::FromUtf8Error),
    InvalidInput(String),
    Rejected,
}

impl fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Protocol(error) => write!(formatter, "protocol error: {error}"),
            Self::InvalidUtf8(_) => write!(formatter, "received text is not valid UTF-8"),
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::Rejected => formatter.write_str("receiver rejected the transfer"),
        }
    }
}

impl std::error::Error for NetworkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::InvalidUtf8(error) => Some(error),
            Self::InvalidInput(_) | Self::Rejected => None,
        }
    }
}

impl From<io::Error> for NetworkError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ProtocolError> for NetworkError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

impl From<std::string::FromUtf8Error> for NetworkError {
    fn from(error: std::string::FromUtf8Error) -> Self {
        Self::InvalidUtf8(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDevice {
    pub name: String,
    pub address: IpAddr,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivedPayload {
    Text {
        text: String,
        peer: IpAddr,
    },
    File {
        path: PathBuf,
        size: u64,
        peer: IpAddr,
    },
}

pub fn send_text<F>(host: &str, port: u16, text: &str, progress: F) -> NetworkResult<()>
where
    F: FnMut(u64, u64),
{
    if text.len() as u64 > MAX_TEXT_BYTES {
        return Err(NetworkError::InvalidInput(format!(
            "text is larger than the {MAX_TEXT_BYTES} byte limit"
        )));
    }

    let header = Header {
        kind: PayloadKind::Text,
        name: "message.txt".to_owned(),
        payload_len: text.len() as u64,
    };
    send_payload(host, port, &header, &mut text.as_bytes(), progress)
}

pub fn send_file<F>(host: &str, port: u16, path: &Path, progress: F) -> NetworkResult<u64>
where
    F: FnMut(u64, u64),
{
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(NetworkError::InvalidInput(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| NetworkError::InvalidInput("file name must be valid UTF-8".to_owned()))?
        .to_owned();
    let header = Header {
        kind: PayloadKind::File,
        name,
        payload_len: metadata.len(),
    };
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    send_payload(host, port, &header, &mut reader, progress)?;
    Ok(metadata.len())
}

pub fn send_payload<R, F>(
    host: &str,
    port: u16,
    header: &Header,
    reader: &mut R,
    progress: F,
) -> NetworkResult<()>
where
    R: Read,
    F: FnMut(u64, u64),
{
    let mut stream = connect(host, port)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    write_header(&mut stream, header)?;
    copy_exact_with_progress(reader, &mut stream, header.payload_len, progress)?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)?;

    let mut acknowledgment = [0_u8; 2];
    stream.read_exact(&mut acknowledgment)?;
    if &acknowledgment != ACK_OK {
        return Err(NetworkError::Rejected);
    }
    Ok(())
}

pub fn receive_one<S, P>(
    mut stream: TcpStream,
    output: &Path,
    mut started: S,
    progress: P,
) -> NetworkResult<ReceivedPayload>
where
    S: FnMut(&Header),
    P: FnMut(u64, u64),
{
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let peer = stream.peer_addr()?.ip();
    let header = match read_header(&mut stream) {
        Ok(header) => header,
        Err(error) => {
            let _ = stream.write_all(ACK_ERROR);
            return Err(error.into());
        }
    };
    started(&header);

    let result = match header.kind {
        PayloadKind::Text => receive_text(&mut stream, peer, &header, progress),
        PayloadKind::File => receive_file(&mut stream, output, peer, &header, progress),
    };

    match result {
        Ok(payload) => {
            stream.write_all(ACK_OK)?;
            stream.flush()?;
            Ok(payload)
        }
        Err(error) => {
            let _ = stream.write_all(ACK_ERROR);
            Err(error)
        }
    }
}

fn receive_text<F>(
    stream: &mut TcpStream,
    peer: IpAddr,
    header: &Header,
    progress: F,
) -> NetworkResult<ReceivedPayload>
where
    F: FnMut(u64, u64),
{
    if header.payload_len > MAX_TEXT_BYTES {
        return Err(NetworkError::InvalidInput(format!(
            "text payload is too large: {} bytes",
            header.payload_len
        )));
    }
    let mut bytes = Vec::with_capacity(header.payload_len as usize);
    copy_exact_with_progress(stream, &mut bytes, header.payload_len, progress)?;
    Ok(ReceivedPayload::Text {
        text: String::from_utf8(bytes)?,
        peer,
    })
}

fn receive_file<F>(
    stream: &mut TcpStream,
    output: &Path,
    peer: IpAddr,
    header: &Header,
    progress: F,
) -> NetworkResult<ReceivedPayload>
where
    F: FnMut(u64, u64),
{
    let destination = unique_destination(output, &header.name)?;
    let temporary = destination.with_extension(format!(
        "{}.part",
        destination
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("lankuu")
    ));

    let receive_result = (|| -> NetworkResult<()> {
        let file = File::create(&temporary)?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        copy_exact_with_progress(stream, &mut writer, header.payload_len, progress)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        fs::rename(&temporary, &destination)?;
        Ok(())
    })();

    if receive_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    receive_result?;
    Ok(ReceivedPayload::File {
        path: destination,
        size: header.payload_len,
        peer,
    })
}

pub fn discover_devices(timeout: Duration) -> NetworkResult<Vec<DiscoveredDevice>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_broadcast(true)?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    socket.send_to(DISCOVERY_QUERY, ("255.255.255.255", DISCOVERY_PORT))?;

    let deadline = Instant::now() + timeout;
    let mut seen = HashSet::new();
    let mut devices = Vec::new();
    let mut buffer = [0_u8; 512];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((length, source)) => {
                let response = String::from_utf8_lossy(&buffer[..length]);
                if let Some((name, port)) = parse_discovery_response(&response) {
                    let key = (source.ip(), port);
                    if seen.insert(key) {
                        devices.push(DiscoveredDevice {
                            name: name.to_owned(),
                            address: source.ip(),
                            port,
                        });
                    }
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
    }
    devices.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    Ok(devices)
}

pub fn spawn_discovery_responder(
    device_name: String,
    transfer_port: u16,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let socket = match UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)) {
            Ok(socket) => socket,
            Err(error) => {
                eprintln!("LanKuu discovery is unavailable: {error}");
                return;
            }
        };
        let _ = socket.set_read_timeout(Some(Duration::from_millis(300)));
        let safe_name = device_name.replace('|', "_");
        let response = format!("{DISCOVERY_RESPONSE_PREFIX}|{safe_name}|{transfer_port}");
        let mut buffer = [0_u8; 128];

        while !stop.load(Ordering::Relaxed) {
            match socket.recv_from(&mut buffer) {
                Ok((length, source)) if &buffer[..length] == DISCOVERY_QUERY => {
                    let _ = socket.send_to(response.as_bytes(), source);
                }
                Ok(_) => {}
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.kind() == io::ErrorKind::TimedOut => {}
                Err(error) => {
                    eprintln!("LanKuu discovery responder stopped: {error}");
                    break;
                }
            }
        }
    })
}

pub fn parse_discovery_response(response: &str) -> Option<(&str, u16)> {
    let mut fields = response.split('|');
    if fields.next()? != DISCOVERY_RESPONSE_PREFIX {
        return None;
    }
    let name = fields.next()?;
    let port = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some((name, port))
}

fn connect(host: &str, port: u16) -> NetworkResult<TcpStream> {
    let addresses = (host, port).to_socket_addrs()?;
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, Duration::from_secs(8)) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(NetworkError::Io(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AddrNotAvailable, "host did not resolve")
    })))
}

fn copy_exact_with_progress<R, W, F>(
    reader: &mut R,
    writer: &mut W,
    expected: u64,
    mut progress: F,
) -> io::Result<u64>
where
    R: Read,
    W: Write,
    F: FnMut(u64, u64),
{
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut copied = 0_u64;
    progress(0, expected);
    while copied < expected {
        let remaining = expected - copied;
        let request = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let read = reader.read(&mut buffer[..request])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("expected {expected} bytes but received {copied}"),
            ));
        }
        writer.write_all(&buffer[..read])?;
        copied += read as u64;
        progress(copied, expected);
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use std::net::TcpListener;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn discovery_response_parses_valid_input() {
        assert_eq!(
            parse_discovery_response("LANKUU_HERE_V1|Ubuntu Laptop|45454"),
            Some(("Ubuntu Laptop", 45_454))
        );
        assert_eq!(parse_discovery_response("wrong|device|45454"), None);
    }

    #[test]
    fn progress_copy_reports_start_and_completion() {
        let mut source = Cursor::new(b"hello");
        let mut output = Vec::new();
        let mut events = Vec::new();
        copy_exact_with_progress(&mut source, &mut output, 5, |sent, total| {
            events.push((sent, total));
        })
        .unwrap();
        assert_eq!(output, b"hello");
        assert_eq!(events.first(), Some(&(0, 5)));
        assert_eq!(events.last(), Some(&(5, 5)));
    }

    #[test]
    fn receiver_accepts_an_ordered_multi_file_batch() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("lankuu-batch-{}-{unique}", std::process::id()));
        let source = root.join("source");
        let output = root.join("output");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&output).unwrap();
        let first = source.join("first.txt");
        let second = source.join("second.bin");
        fs::write(&first, b"first payload").unwrap();
        fs::write(&second, [1_u8, 2, 3, 4, 5]).unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let receiver_output = output.clone();
        let receiver = thread::spawn(move || {
            let mut received = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                received.push(receive_one(stream, &receiver_output, |_| {}, |_, _| {}).unwrap());
            }
            received
        });

        send_file(&address.ip().to_string(), address.port(), &first, |_, _| {}).unwrap();
        send_file(
            &address.ip().to_string(),
            address.port(),
            &second,
            |_, _| {},
        )
        .unwrap();
        let received = receiver.join().unwrap();

        assert_eq!(received.len(), 2);
        assert_eq!(
            fs::read(output.join("first.txt")).unwrap(),
            b"first payload"
        );
        assert_eq!(
            fs::read(output.join("second.bin")).unwrap(),
            [1_u8, 2, 3, 4, 5]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
