// SPDX-License-Identifier: GPL-3.0-only

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use freeshow_core::{ICECAST_BUS, IcecastDestination};
use gst::prelude::*;
use gstreamer as gst;

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
        let Ok(pipeline) = pipeline(&destination) else {
            tracing::warn!("Icecast needs GStreamer Opus, Ogg and shout2send plugins");
            continue;
        };
        let bus = pipeline.bus().expect("pipeline bus");
        let running = pipeline.set_state(gst::State::Playing).is_ok();
        let mut disconnected = false;
        let mut interrupted = false;
        if running {
            loop {
                match commands.recv_timeout(Duration::from_millis(100)) {
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
            // Avoid logging libshout errors that may include connection details.
            tracing::warn!("Icecast connection ended. Retrying in three seconds");
            match commands.recv_timeout(Duration::from_secs(3)) {
                Ok(update) => desired = update,
                Err(RecvTimeoutError::Timeout) => desired = Some(destination),
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn pipeline(destination: &IcecastDestination) -> Result<gst::Pipeline, Box<dyn std::error::Error>> {
    gst::init()?;
    let description = format!(
        "{} ! audioconvert ! opusenc bitrate=96000 ! oggmux max-delay=100000000 max-page-delay=100000000 ! shout2send name=icecast timeout=2000",
        freeshow_media::network_audio_mix(ICECAST_BUS),
    );
    let pipeline = gst::parse::launch(&description)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "expected an Icecast pipeline")?;
    let sink = pipeline.by_name("icecast").ok_or("Icecast sink missing")?;
    sink.set_property("ip", &destination.host);
    sink.set_property("port", i32::from(destination.port));
    sink.set_property("mount", &destination.mount);
    sink.set_property("password", &destination.password);
    sink.set_property("streamname", "FreeShow Audio Stream");
    Ok(pipeline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    #[test]
    fn sends_authenticated_stereo_ogg_opus_and_disconnects_on_disable() {
        gst::init().unwrap();
        // Some Windows SDK builds omit libshout. The app can run without it.
        if gst::ElementFactory::find("shout2send").is_none() {
            if cfg!(windows) {
                return;
            }
            panic!("expected shout2send on the native test host");
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (opus, audio) = mpsc::sync_channel(1);
        let (end, closed) = mpsc::sync_channel(1);
        let receiver = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(6);
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
                if headers.starts_with("OPTIONS ") || !headers.contains("Authorization:") {
                    stream
                        .get_mut()
                        .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
                        .unwrap();
                    continue;
                }
                let request = headers.lines().next().unwrap();
                assert!(
                    request.starts_with("SOURCE /service.opus ")
                        || request.starts_with("PUT /service.opus ")
                );
                assert!(headers.contains("Authorization: Basic c291cmNlOnRlc3Q="));
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("content-type: application/ogg")
                );
                let status = if headers
                    .to_ascii_lowercase()
                    .contains("expect: 100-continue")
                {
                    b"HTTP/1.1 100 Continue\r\n\r\n".as_slice()
                } else {
                    b"HTTP/1.1 200 OK\r\n\r\n".as_slice()
                };
                stream.get_mut().write_all(status).unwrap();
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
        let sender = IcecastSender::new().unwrap();
        sender.update(Some(IcecastDestination {
            host: "127.0.0.1".into(),
            port,
            mount: "/service.opus".into(),
            password: "test".into(),
        }));
        let bytes = audio.recv_timeout(Duration::from_secs(7)).unwrap();
        assert_eq!(&bytes[..4], b"OggS");
        assert_eq!(&bytes[28..36], b"OpusHead");
        assert_eq!(bytes[37], 2);
        sender.update(None);
        closed.recv_timeout(Duration::from_secs(5)).unwrap();
        receiver.join().unwrap();
    }
}
