use lankuu_core::{
    copy_exact, read_header, unique_destination, write_header, Header, PayloadKind, ACK_ERROR,
    ACK_OK, DEFAULT_TRANSFER_PORT, DISCOVERY_PORT, DISCOVERY_QUERY, DISCOVERY_RESPONSE_PREFIX,
    MAX_TEXT_BYTES,
};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

const USAGE: &str = r#"LanKuu — local-network file and text sharing

Usage:
  lankuu receive [--bind ADDRESS] [--output DIRECTORY] [--name DEVICE] [--once]
  lankuu send HOST FILE [--port PORT]
  lankuu send-text HOST TEXT [--port PORT]
  lankuu discover [--timeout SECONDS]
  lankuu help

Examples:
  lankuu receive --output ~/Downloads/LanKuu
  lankuu discover
  lankuu send 192.168.1.42 ./movie.mkv
  lankuu send-text 192.168.1.42 "hello from Ubuntu"

MVP note: traffic is not encrypted yet. Use LanKuu only on a trusted LAN.
"#;

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> AppResult<()> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let Some(command) = arguments.first().map(String::as_str) else {
        print!("{USAGE}");
        return Ok(());
    };

    match command {
        "receive" => receive_command(&arguments[1..]),
        "send" => send_file_command(&arguments[1..]),
        "send-text" => send_text_command(&arguments[1..]),
        "discover" => discover_command(&arguments[1..]),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("lankuu {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        unknown => Err(format!("unknown command '{unknown}'\n\n{USAGE}").into()),
    }
}

fn receive_command(arguments: &[String]) -> AppResult<()> {
    let mut bind = format!("0.0.0.0:{DEFAULT_TRANSFER_PORT}");
    let mut output = default_download_directory();
    let mut device_name = hostname();
    let mut once = false;

    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--bind" => {
                bind = required_value(arguments, &mut index, "--bind")?;
            }
            "--output" => {
                output = PathBuf::from(required_value(arguments, &mut index, "--output")?);
            }
            "--name" => {
                device_name = required_value(arguments, &mut index, "--name")?;
            }
            "--once" => once = true,
            unknown => return Err(format!("unknown receive option '{unknown}'").into()),
        }
        index += 1;
    }

    fs::create_dir_all(&output)?;
    let listener = TcpListener::bind(&bind)?;
    let port = listener.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let discovery = spawn_discovery_responder(device_name, port, Arc::clone(&stop));

    println!("LanKuu is listening on {bind}");
    println!("Files will be saved to {}", output.display());
    println!("Press Ctrl+C to stop.");

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let peer = stream.peer_addr()?;
                println!("Incoming connection from {peer}");
                if let Err(error) = receive_one(stream, &output) {
                    eprintln!("Transfer from {peer} failed: {error}");
                }
                if once {
                    break;
                }
            }
            Err(error) => eprintln!("Connection failed: {error}"),
        }
    }

    stop.store(true, Ordering::Relaxed);
    if once {
        let _ = discovery.join();
    }
    Ok(())
}

fn receive_one(mut stream: TcpStream, output: &Path) -> AppResult<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let header = match read_header(&mut stream) {
        Ok(header) => header,
        Err(error) => {
            let _ = stream.write_all(ACK_ERROR);
            return Err(error.into());
        }
    };

    let result: AppResult<()> = match header.kind {
        PayloadKind::Text => {
            if header.payload_len > MAX_TEXT_BYTES {
                Err(format!("text payload is too large: {} bytes", header.payload_len).into())
            } else {
                let mut bytes = Vec::with_capacity(header.payload_len as usize);
                copy_exact(&mut stream, &mut bytes, header.payload_len)?;
                let text = String::from_utf8(bytes)?;
                println!("\nText received:\n{text}\n");
                Ok(())
            }
        }
        PayloadKind::File => receive_file(&mut stream, output, &header),
    };

    match result {
        Ok(()) => {
            stream.write_all(ACK_OK)?;
            stream.flush()?;
            Ok(())
        }
        Err(error) => {
            let _ = stream.write_all(ACK_ERROR);
            Err(error)
        }
    }
}

fn receive_file(stream: &mut TcpStream, output: &Path, header: &Header) -> AppResult<()> {
    let destination = unique_destination(output, &header.name)?;
    let temporary = destination.with_extension(format!(
        "{}.part",
        destination
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("lankuu")
    ));

    let receive_result = (|| -> AppResult<()> {
        let file = File::create(&temporary)?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        let received = copy_exact(stream, &mut writer, header.payload_len)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        fs::rename(&temporary, &destination)?;
        println!("Received {} ({} bytes)", destination.display(), received);
        Ok(())
    })();

    if receive_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    receive_result
}

fn send_file_command(arguments: &[String]) -> AppResult<()> {
    let (positionals, port) = parse_port_option(arguments)?;
    if positionals.len() != 2 {
        return Err("usage: lankuu send HOST FILE [--port PORT]".into());
    }

    let host = &positionals[0];
    let path = PathBuf::from(&positionals[1]);
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()).into());
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("file name must be valid UTF-8")?
        .to_owned();

    let file = File::open(&path)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let header = Header {
        kind: PayloadKind::File,
        name,
        payload_len: metadata.len(),
    };

    println!(
        "Sending {} ({} bytes) to {host}…",
        path.display(),
        metadata.len()
    );
    send_payload(host, port, &header, &mut reader)?;
    println!("Transfer complete.");
    Ok(())
}

fn send_text_command(arguments: &[String]) -> AppResult<()> {
    let (positionals, port) = parse_port_option(arguments)?;
    if positionals.len() < 2 {
        return Err("usage: lankuu send-text HOST TEXT [--port PORT]".into());
    }

    let host = &positionals[0];
    let text = positionals[1..].join(" ");
    if text.len() as u64 > MAX_TEXT_BYTES {
        return Err(format!("text is larger than the {} byte limit", MAX_TEXT_BYTES).into());
    }
    let header = Header {
        kind: PayloadKind::Text,
        name: "message.txt".to_owned(),
        payload_len: text.len() as u64,
    };
    let mut bytes = text.as_bytes();

    send_payload(host, port, &header, &mut bytes)?;
    println!("Text delivered.");
    Ok(())
}

fn send_payload<R: Read>(host: &str, port: u16, header: &Header, reader: &mut R) -> AppResult<()> {
    let mut addresses = (host, port).to_socket_addrs()?;
    let address = addresses
        .next()
        .ok_or("host did not resolve to an address")?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(8))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    write_header(&mut stream, header)?;
    copy_exact(reader, &mut stream, header.payload_len)?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)?;

    let mut acknowledgment = [0_u8; 2];
    stream.read_exact(&mut acknowledgment)?;
    if &acknowledgment != ACK_OK {
        return Err("receiver rejected the transfer".into());
    }
    Ok(())
}

fn discover_command(arguments: &[String]) -> AppResult<()> {
    let mut timeout = 3_u64;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--timeout" => {
                timeout = required_value(arguments, &mut index, "--timeout")?.parse()?;
            }
            unknown => return Err(format!("unknown discover option '{unknown}'").into()),
        }
        index += 1;
    }

    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_broadcast(true)?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    socket.send_to(DISCOVERY_QUERY, ("255.255.255.255", DISCOVERY_PORT))?;

    println!("Searching for LanKuu devices…");
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let mut found = Vec::new();
    let mut buffer = [0_u8; 512];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((length, source)) => {
                let response = String::from_utf8_lossy(&buffer[..length]);
                if let Some((name, port)) = parse_discovery_response(&response) {
                    let key = (source.ip(), port);
                    if !found.contains(&key) {
                        found.push(key);
                        println!("{name}\t{}:{port}", source.ip());
                    }
                }
            }
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
    }

    if found.is_empty() {
        println!("No devices found. Make sure the receiver is running on the same LAN.");
    }
    Ok(())
}

fn spawn_discovery_responder(
    device_name: String,
    transfer_port: u16,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let socket = match UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)) {
            Ok(socket) => socket,
            Err(error) => {
                eprintln!("Discovery is unavailable: {error}");
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
                    eprintln!("Discovery responder stopped: {error}");
                    break;
                }
            }
        }
    })
}

fn parse_discovery_response(response: &str) -> Option<(&str, u16)> {
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

fn parse_port_option(arguments: &[String]) -> AppResult<(Vec<String>, u16)> {
    let mut port = DEFAULT_TRANSFER_PORT;
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--port" {
            port = required_value(arguments, &mut index, "--port")?.parse()?;
        } else if arguments[index].starts_with('-') {
            return Err(format!("unknown option '{}'", arguments[index]).into());
        } else {
            positionals.push(arguments[index].clone());
        }
        index += 1;
    }
    Ok((positionals, port))
}

fn required_value(arguments: &[String], index: &mut usize, option: &str) -> AppResult<String> {
    *index += 1;
    arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| format!("{option} requires a value").into())
}

fn default_download_directory() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("LanKuu")
}

fn hostname() -> String {
    env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Ubuntu".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_response_round_trip() {
        let response = "LANKUU_HERE_V1|Ubuntu Laptop|45454";
        assert_eq!(
            parse_discovery_response(response),
            Some(("Ubuntu Laptop", 45_454))
        );
    }

    #[test]
    fn port_option_can_appear_between_positionals() {
        let arguments = vec![
            "host".to_owned(),
            "--port".to_owned(),
            "1234".to_owned(),
            "file".to_owned(),
        ];
        let (positionals, port) = parse_port_option(&arguments).unwrap();
        assert_eq!(positionals, ["host", "file"]);
        assert_eq!(port, 1234);
    }
}
