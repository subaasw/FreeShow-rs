// SPDX-License-Identifier: GPL-3.0-only

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::STANDARD};
use freeshow_core::{ICECAST_BUS, IcecastDestination};
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app::AppSink;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};

const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// Owns a single sender so changing settings cannot race an old connection
/// for the mount. Pipeline creation, network waits, and teardown stay off the UI.
pub struct IcecastSender {
    commands: Option<Sender<Option<IcecastDestination>>>,
    worker: Option<JoinHandle<()>>,
}

impl IcecastSender {
    pub fn new() -> std::io::Result<Self> {
        let (send, receive) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("freeshow-icecast".into())
            .spawn(move || serve(receive))?;
        Ok(Self {
            commands: Some(send),
            worker: Some(worker),
        })
    }

    pub fn update(&self, destination: Option<IcecastDestination>) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(destination);
        }
    }
}

impl Drop for IcecastSender {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn serve(commands: Receiver<Option<IcecastDestination>>) {
    let mut desired = None;
    loop {
        let Some(destination) = desired.take() else {
            match commands.recv() {
                Ok(update) => desired = update,
                Err(_) => return,
            }
            continue;
        };
        let Ok((pipeline, sink)) = pipeline(&destination) else {
            tracing::warn!("Icecast needs GStreamer stereo mixing, Opus, and Ogg plugins");
            continue;
        };
        let bus = pipeline.bus().expect("pipeline bus");
        let mut disconnected = false;
        let mut interrupted = false;
        let stream = sink
            .as_ref()
            .map(|_| connect_source(&destination))
            .transpose();
        if let Ok(mut stream) = stream
            && pipeline.set_state(gst::State::Playing).is_ok()
        {
            loop {
                let update = if let Some((sink, stream)) = sink.as_ref().zip(stream.as_mut()) {
                    match commands.try_recv() {
                        Ok(update) => Ok(update),
                        Err(TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
                        Err(TryRecvError::Empty) => {
                            if let Some(sample) =
                                sink.try_pull_sample(gst::ClockTime::from_mseconds(100))
                                && write_sample(stream.writer(), &sample).is_err()
                            {
                                break;
                            }
                            Err(RecvTimeoutError::Timeout)
                        }
                    }
                } else {
                    commands.recv_timeout(Duration::from_millis(100))
                };
                match update {
                    Ok(update) => {
                        desired = update;
                        interrupted = true;
                        break;
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
                if bus
                    .pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos])
                    .is_some()
                {
                    break;
                }
            }
            if (interrupted || disconnected)
                && let Some(stream) = stream.as_mut()
            {
                stream.close();
            }
        }
        let _ = pipeline.set_state(gst::State::Null);
        if disconnected {
            return;
        }
        if !interrupted {
            // Network errors may contain sensitive destination details.
            tracing::warn!("Icecast connection ended. Retrying in three seconds");
            match commands.recv_timeout(Duration::from_secs(3)) {
                Ok(update) => desired = update,
                Err(RecvTimeoutError::Timeout) => desired = Some(destination),
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn pipeline(
    destination: &IcecastDestination,
) -> Result<(gst::Pipeline, Option<AppSink>), Box<dyn std::error::Error>> {
    gst::init()?;
    let libshout = destination.tls.is_none() && gst::ElementFactory::find("shout2send").is_some();
    let sink = if libshout {
        "shout2send name=icecast timeout=2000"
    } else {
        "appsink name=icecast max-buffers=16 sync=false wait-on-eos=false"
    };
    let description = format!(
        "{} ! audioconvert ! opusenc bitrate=96000 ! oggmux max-delay=100000000 max-page-delay=100000000 ! {sink}",
        freeshow_media::network_audio_mix(ICECAST_BUS),
    );
    let pipeline = gst::parse::launch(&description)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "expected an Icecast pipeline")?;
    let sink = pipeline.by_name("icecast").ok_or("Icecast sink missing")?;
    if libshout {
        sink.set_property("ip", &destination.host);
        sink.set_property("port", i32::from(destination.port));
        sink.set_property("mount", &destination.mount);
        sink.set_property("password", &destination.password);
        sink.set_property("streamname", "FreeShow Audio Stream");
        Ok((pipeline, None))
    } else {
        Ok((
            pipeline,
            Some(sink.downcast().map_err(|_| "expected an appsink")?),
        ))
    }
}

enum SourceConnection {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, StartupSocket>>),
}

impl SourceConnection {
    fn writer(&mut self) -> &mut dyn Write {
        match self {
            Self::Plain(socket) => socket,
            Self::Tls(socket) => socket.as_mut(),
        }
    }

    fn close(&mut self) {
        if let Self::Tls(socket) = self {
            socket.conn.send_close_notify();
            let _ = socket.flush();
        }
    }
}

fn connect_source(destination: &IcecastDestination) -> io::Result<SourceConnection> {
    connect_source_with(destination, || {
        static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
        CONFIG
            .get_or_init(|| {
                tls_config(RootCertStore::from_iter(
                    webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
                ))
            })
            .clone()
    })
}

fn tls_config(roots: RootCertStore) -> Arc<ClientConfig> {
    Arc::new(
        ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
            .with_safe_default_protocol_versions()
            .expect("ring supports the default TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

fn connect_source_with(
    destination: &IcecastDestination,
    config: impl FnOnce() -> Arc<ClientConfig>,
) -> io::Result<SourceConnection> {
    let host = destination.host.as_str();
    let mount = destination.mount.as_str();
    if host.is_empty()
        || host.contains(['/', '@', '?', '#', '\\'])
        || !mount.starts_with('/')
        || mount.contains(['?', '#'])
        || [host, mount]
            .into_iter()
            .any(|part| part.chars().any(|ch| ch.is_control() || ch.is_whitespace()))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Icecast source address",
        ));
    }
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let port = destination.port;
    let addresses = (host, port).to_socket_addrs()?;
    let deadline = Instant::now() + IO_TIMEOUT;
    let stream = addresses
        .take_while(|_| Instant::now() < deadline)
        .find_map(|address| TcpStream::connect_timeout(&address, remaining(deadline).ok()?).ok())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "could not connect to Icecast")
        })?;
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut socket = StartupSocket {
        stream,
        deadline: Some(Instant::now() + IO_TIMEOUT),
    };
    if destination.tls == Some(true) {
        let name =
            ServerName::try_from(host.to_owned()).map_err(|_| io::ErrorKind::InvalidInput)?;
        let connection = ClientConnection::new(config(), name).map_err(io::Error::other)?;
        let mut stream = StreamOwned::new(connection, socket);
        authenticate_source(&mut stream, mount, &authority, &destination.password)?;
        stream.sock.start_streaming()?;
        Ok(SourceConnection::Tls(Box::new(stream)))
    } else {
        authenticate_source(&mut socket, mount, &authority, &destination.password)?;
        socket.start_streaming()?;
        Ok(SourceConnection::Plain(socket.stream))
    }
}

// Rustls can perform multiple reads and writes inside one handshake or flush.
struct StartupSocket {
    stream: TcpStream,
    deadline: Option<Instant>,
}

impl StartupSocket {
    fn start_streaming(&mut self) -> io::Result<()> {
        self.stream.set_read_timeout(Some(IO_TIMEOUT))?;
        self.stream.set_write_timeout(Some(IO_TIMEOUT))?;
        self.deadline = None;
        Ok(())
    }
}

impl Read for StartupSocket {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(deadline) = self.deadline {
            self.stream.set_read_timeout(Some(remaining(deadline)?))?;
        }
        self.stream.read(buffer)
    }
}

impl Write for StartupSocket {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if let Some(deadline) = self.deadline {
            self.stream.set_write_timeout(Some(remaining(deadline)?))?;
        }
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

fn authenticate_source(
    mut stream: impl Read + Write,
    mount: &str,
    authority: &str,
    password: &str,
) -> io::Result<()> {
    let auth = STANDARD.encode(format!("source:{password}"));
    // SOURCE streams a raw body and is accepted by Icecast 2.4 and 2.5.
    write!(
        stream,
        "SOURCE {mount} HTTP/1.0\r\nHost: {authority}\r\nAuthorization: Basic {auth}\r\nContent-Type: application/ogg\r\nIce-Name: FreeShow Audio Stream\r\nIce-Public: 0\r\n\r\n"
    )?;
    stream.flush()?;
    let mut status = [0; 1024];
    let mut used = 0;
    while used < status.len() {
        let count = stream.read(&mut status[used..])?;
        if count == 0 {
            break;
        }
        if let Some(end) = status[used..used + count]
            .iter()
            .position(|byte| *byte == b'\n')
        {
            let mut fields = status[..used + end]
                .split(|byte| byte.is_ascii_whitespace())
                .filter(|field| !field.is_empty());
            if matches!(fields.next(), Some(b"HTTP/1.0" | b"HTTP/1.1"))
                && fields.next() == Some(b"200")
            {
                return Ok(());
            }
            break;
        }
        used += count;
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Icecast rejected the source",
    ))
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or(io::ErrorKind::TimedOut.into())
}

fn write_sample(stream: &mut dyn Write, sample: &gst::Sample) -> io::Result<()> {
    let buffer = sample.buffer().ok_or(io::ErrorKind::InvalidData)?;
    let map = buffer
        .map_readable()
        .map_err(|_| io::ErrorKind::InvalidData)?;
    stream.write_all(map.as_slice())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::{ServerConfig, ServerConnection, pki_types::PrivatePkcs8KeyDer};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    #[test]
    fn tls_source_authenticates_only_after_verifying_the_server() {
        // The fixture key is public test material for localhost only.
        let ca = include_bytes!("../tests/fixtures/icecast-test-ca.der");
        let certificate = include_bytes!("../tests/fixtures/icecast-test-cert.der");
        let key = include_bytes!("../tests/fixtures/icecast-test-key.der");
        let mut roots = RootCertStore::empty();
        roots.add(ca.as_slice().into()).unwrap();
        let client = tls_config(roots);
        let server = Arc::new(
            ServerConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![certificate.as_slice().into()],
                    PrivatePkcs8KeyDer::from(key.as_slice()).into(),
                )
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let receiver = thread::spawn(move || {
            for authorized in [false, false, true] {
                let (stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
                let connection = ServerConnection::new(Arc::clone(&server)).unwrap();
                let mut stream = BufReader::new(StreamOwned::new(connection, stream));
                let mut request = String::new();
                let read = stream.read_line(&mut request);
                if !authorized {
                    assert!(request.is_empty());
                    continue;
                }
                read.unwrap();
                assert_eq!(request, "SOURCE /secure.opus HTTP/1.0\r\n");
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    assert_ne!(stream.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    headers.push_str(&line);
                }
                assert!(headers.contains("Authorization: Basic c291cmNlOnRlc3Q="));
                stream
                    .get_mut()
                    .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
                    .unwrap();
                stream.get_mut().flush().unwrap();
                let mut body = Vec::new();
                stream.read_to_end(&mut body).unwrap();
                assert_eq!(body, b"OggS");
            }
        });

        let mut destination = IcecastDestination {
            host: "localhost".into(),
            port,
            mount: "/secure.opus".into(),
            password: "test".into(),
            tls: Some(true),
        };
        assert!(
            connect_source(&destination)
                .is_err_and(|error| error.kind() == io::ErrorKind::InvalidData)
        );
        destination.host = "127.0.0.1".into();
        assert!(
            connect_source_with(&destination, || Arc::clone(&client))
                .is_err_and(|error| error.kind() == io::ErrorKind::InvalidData)
        );
        destination.host = "localhost".into();
        let mut stream = connect_source_with(&destination, || client).unwrap();
        stream.writer().write_all(b"OggS").unwrap();
        stream.close();
        receiver.join().unwrap();
    }

    #[test]
    fn sends_authenticated_stereo_ogg_opus_and_disconnects_on_disable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (opus, audio) = mpsc::sync_channel(1);
        let (end, closed) = mpsc::sync_channel(1);
        let receiver = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(6);
            let mut rejected = false;
            loop {
                assert!(Instant::now() < deadline, "Icecast source did not connect");
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(err) => panic!("Icecast listener: {err}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(4)))
                    .unwrap();
                let mut stream = BufReader::new(&mut socket);
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    if stream.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    headers.push_str(&line);
                }
                let request = headers.lines().next().unwrap();
                assert_eq!(request, "SOURCE /service.opus HTTP/1.0");
                if !rejected {
                    assert!(headers.contains("Authorization: Basic c291cmNlOndyb25n"));
                    stream
                        .get_mut()
                        .write_all(b"HTTP/1.0 401 Unauthorized\r\n\r\n")
                        .unwrap();
                    assert_eq!(stream.read(&mut [0]).unwrap(), 0);
                    rejected = true;
                    continue;
                }
                assert!(headers.contains("Authorization: Basic c291cmNlOnRlc3Q="));
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("content-type: application/ogg")
                );
                stream
                    .get_mut()
                    .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
                    .unwrap();
                let mut first = [0; 47];
                stream.read_exact(&mut first).unwrap();
                // Read a media page after the identification and comment headers.
                loop {
                    let mut header = [0; 27];
                    stream.read_exact(&mut header).unwrap();
                    assert_eq!(&header[..4], b"OggS");
                    let mut segments = vec![0; usize::from(header[26])];
                    stream.read_exact(&mut segments).unwrap();
                    let size: usize = segments.iter().map(|segment| usize::from(*segment)).sum();
                    let mut data = vec![0; size];
                    stream.read_exact(&mut data).unwrap();
                    let granule = u64::from_le_bytes(header[6..14].try_into().unwrap());
                    if granule != 0 && granule != u64::MAX && size > 0 {
                        break;
                    }
                }
                opus.send(first).unwrap();
                let mut remaining = Vec::new();
                stream.read_to_end(&mut remaining).unwrap();
                end.send(()).unwrap();
                return;
            }
        });
        let mut destination = IcecastDestination {
            host: "127.0.0.1".into(),
            port,
            mount: "/service.opus".into(),
            password: "wrong".into(),
            tls: Some(false),
        };
        assert!(
            connect_source(&destination)
                .is_err_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
        );
        destination.password = "test".into();
        let sender = IcecastSender::new().unwrap();
        sender.update(Some(destination));
        let bytes = audio.recv_timeout(Duration::from_secs(7)).unwrap();
        assert_eq!(&bytes[..4], b"OggS");
        assert_eq!(&bytes[28..36], b"OpusHead");
        assert_eq!(bytes[37], 2);
        sender.update(None);
        closed.recv_timeout(Duration::from_secs(5)).unwrap();
        receiver.join().unwrap();
    }
}
