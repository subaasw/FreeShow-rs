// SPDX-License-Identifier: GPL-3.0-only

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::STANDARD};
use freeshow_core::{ICECAST_BUS, IcecastDestination};
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app::AppSink;

const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// Owns a single sender so changing settings cannot race an old connection
/// for the mount. Pipeline creation, network waits, and teardown stay off the UI.
pub struct IcecastSender {
    commands: Option<Sender<Option<IcecastDestination>>>,
    worker: Option<JoinHandle<()>>,
}

impl IcecastSender {
    pub fn new() -> std::io::Result<Self> {
        Self::spawn(true)
    }

    fn spawn(prefer_libshout: bool) -> std::io::Result<Self> {
        let (send, receive) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("freeshow-icecast".into())
            .spawn(move || serve(receive, prefer_libshout))?;
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

fn serve(commands: Receiver<Option<IcecastDestination>>, prefer_libshout: bool) {
    let mut desired = None;
    loop {
        let Some(destination) = desired.take() else {
            match commands.recv() {
                Ok(update) => desired = update,
                Err(_) => return,
            }
            continue;
        };
        let Ok((pipeline, sink)) = pipeline(&destination, prefer_libshout) else {
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
                                && write_sample(stream, &sample).is_err()
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
    prefer_libshout: bool,
) -> Result<(gst::Pipeline, Option<AppSink>), Box<dyn std::error::Error>> {
    gst::init()?;
    let libshout = prefer_libshout && gst::ElementFactory::find("shout2send").is_some();
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

fn connect_source(destination: &IcecastDestination) -> io::Result<TcpStream> {
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
    let mut stream = addresses
        .take_while(|_| Instant::now() < deadline)
        .find_map(|address| TcpStream::connect_timeout(&address, remaining(deadline).ok()?).ok())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "could not connect to Icecast")
        })?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let auth = STANDARD.encode(format!("source:{}", destination.password));
    // SOURCE streams a raw body and is accepted by Icecast 2.4 and 2.5.
    write!(
        stream,
        "SOURCE {mount} HTTP/1.0\r\nHost: {authority}\r\nAuthorization: Basic {auth}\r\nContent-Type: application/ogg\r\nIce-Name: FreeShow Audio Stream\r\nIce-Public: 0\r\n\r\n"
    )?;
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut status = [0; 1024];
    let mut used = 0;
    while used < status.len() {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
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
                return Ok(stream);
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

fn write_sample(stream: &mut TcpStream, sample: &gst::Sample) -> io::Result<()> {
    let buffer = sample.buffer().ok_or(io::ErrorKind::InvalidData)?;
    let map = buffer
        .map_readable()
        .map_err(|_| io::ErrorKind::InvalidData)?;
    stream.write_all(map.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

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
        };
        assert_eq!(
            connect_source(&destination).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        destination.password = "test".into();
        let sender = IcecastSender::spawn(false).unwrap();
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
