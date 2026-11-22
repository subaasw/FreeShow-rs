// SPDX-License-Identifier: GPL-3.0-only

use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gstreamer::glib;
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;
use gstreamer_sdp::SDPMessage;
use gstreamer_webrtc::{WebRTCICEGatheringState, WebRTCSDPType, WebRTCSessionDescription};

use crate::whip::whip_post;
use crate::{Job, encoders, init_gstreamer};

const HANDSHAKE: Duration = Duration::from_secs(8);

/// Sends one WHIP destination with H.264 video and optional routed Opus audio.
/// The show keeps running when the encoder, endpoint, or handshake fails.
pub struct WebrtcSender {
    width: u32,
    height: u32,
    jobs: SyncSender<Job>,
    thread: Option<JoinHandle<()>>,
}

impl WebrtcSender {
    pub fn open(url: &str, token: Option<&str>, width: u32, height: u32) -> Option<Self> {
        Self::open_output(url, token, width, height, None)
    }

    pub fn open_routed(
        url: &str,
        token: Option<&str>,
        width: u32,
        height: u32,
        output: &str,
    ) -> Option<Self> {
        Self::open_output(url, token, width, height, Some(output))
    }

    fn open_output(
        url: &str,
        token: Option<&str>,
        width: u32,
        height: u32,
        output: Option<&str>,
    ) -> Option<Self> {
        let url = url.to_string();
        let token = token.map(str::to_string);
        let output = output.map(str::to_string);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (jobs, jobs_rx) = mpsc::sync_channel(2);
        let thread = std::thread::Builder::new()
            .name("freeshow-webrtc".into())
            .spawn(move || {
                match start_session(&url, token.as_deref(), width, height, output.as_deref()) {
                    Ok((pipeline, appsrc)) => {
                        let _ = ready_tx.send(true);
                        serve(pipeline, appsrc, jobs_rx);
                    }
                    Err(err) => {
                        tracing::warn!("WebRTC output did not start: {err}");
                        let _ = ready_tx.send(false);
                    }
                }
            })
            .ok()?;
        if ready_rx
            .recv_timeout(HANDSHAKE + Duration::from_secs(5))
            .ok()
            != Some(true)
        {
            let _ = jobs.send(Job::Stop);
            let _ = thread.join();
            return None;
        }
        Some(Self {
            width,
            height,
            jobs,
            thread: Some(thread),
        })
    }

    pub fn submit(&self, pixels: &[u8], width: u32, height: u32) {
        if self.width != width || self.height != height {
            return;
        }
        let _ = self.jobs.try_send(Job::Frame(pixels.to_vec()));
    }
}

impl Drop for WebrtcSender {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.jobs.send(Job::Stop);
            let _ = thread.join();
        }
    }
}

fn start_session(
    url: &str,
    token: Option<&str>,
    width: u32,
    height: u32,
    output: Option<&str>,
) -> Result<(gstreamer::Pipeline, AppSrc), String> {
    let (pipeline, appsrc, webrtc) = open_encode_pipeline(width, height, output)?;
    let negotiate = || {
        let offer = local_offer(&webrtc)?;
        let (answer, _) = whip_post(url, token, &offer)?;
        apply_remote_answer(&webrtc, &answer)
    };
    if let Err(error) = negotiate() {
        let _ = pipeline.set_state(gstreamer::State::Null);
        return Err(error);
    }
    tracing::info!("WebRTC WHIP session started");
    Ok((pipeline, appsrc))
}

fn open_encode_pipeline(
    width: u32,
    height: u32,
    output: Option<&str>,
) -> Result<(gstreamer::Pipeline, AppSrc, gstreamer::Element), String> {
    init_gstreamer()?;
    if gstreamer::ElementFactory::find("webrtcbin").is_none() {
        return Err("webrtcbin is not installed".into());
    }
    let pay = "h264parse config-interval=-1 ! rtph264pay pt=96 config-interval=-1 aggregate-mode=zero-latency ! application/x-rtp,media=video,encoding-name=H264,payload=96";
    let audio = output.map_or_else(String::new, |id| format!("{} ! send.", audio_rtp(id)));
    let descriptions: Vec<String> = encoders()
        .into_iter()
        .map(|encoder| {
            format!(
                "webrtcbin name=send bundle-policy=max-bundle appsrc name=src is-live=true format=time block=false do-timestamp=true caps=video/x-raw,format=RGBA,width={width},height={height},framerate=30/1 ! videoconvert ! video/x-raw,format=I420 ! {encoder} ! {pay} ! send. {audio}"
            )
        })
        .collect();
    for description in descriptions {
        let Ok(element) = gstreamer::parse::launch(&description) else {
            continue;
        };
        let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else {
            continue;
        };
        let Some(appsrc) = pipeline
            .by_name("src")
            .and_then(|element| element.dynamic_cast::<AppSrc>().ok())
        else {
            let _ = pipeline.set_state(gstreamer::State::Null);
            continue;
        };
        let Some(webrtc) = pipeline.by_name("send") else {
            let _ = pipeline.set_state(gstreamer::State::Null);
            continue;
        };
        if pipeline.set_state(gstreamer::State::Playing).is_err() {
            let _ = pipeline.set_state(gstreamer::State::Null);
            continue;
        }
        return Ok((pipeline, appsrc, webrtc));
    }
    Err("WebRTC encode pipeline did not start".into())
}

fn audio_rtp(output: &str) -> String {
    format!(
        "{} ! audioconvert ! opusenc bitrate=128000 ! rtpopuspay pt=97 ! application/x-rtp,media=audio,encoding-name=OPUS,clock-rate=48000,encoding-params=(string)2,sprop-stereo=(string)1,payload=97",
        freeshow_media::network_audio_mix(output),
    )
}

fn local_offer(webrtc: &gstreamer::Element) -> Result<String, String> {
    let (tx, rx) = mpsc::channel();
    let promise = gstreamer::Promise::with_change_func(move |reply| {
        let result = match reply {
            Ok(Some(structure)) => structure
                .get::<WebRTCSessionDescription>("offer")
                .map_err(|err| format!("create-offer missing SDP: {err}")),
            Ok(None) => Err("create-offer returned no SDP".into()),
            Err(err) => Err(format!("create-offer failed: {err:?}")),
        };
        let _ = tx.send(result);
    });
    webrtc.emit_by_name::<()>("create-offer", &[&None::<gstreamer::Structure>, &promise]);
    let offer = recv_result(&rx, HANDSHAKE, "create-offer")?;
    let (tx, rx) = mpsc::channel();
    let promise = gstreamer::Promise::with_change_func(move |reply| {
        let _ = tx.send(reply.is_ok());
    });
    webrtc.emit_by_name::<()>("set-local-description", &[&offer, &promise]);
    if recv_optional(&rx, HANDSHAKE).is_none() {
        return Err("set-local-description timed out".into());
    }
    wait_ice(webrtc)?;
    sdp_text(webrtc, "local-description")
}

fn apply_remote_answer(webrtc: &gstreamer::Element, answer: &str) -> Result<(), String> {
    let description = session_description(WebRTCSDPType::Answer, answer)?;
    let (tx, rx) = mpsc::channel();
    let promise = gstreamer::Promise::with_change_func(move |reply| {
        let _ = tx.send(reply.is_ok());
    });
    webrtc.emit_by_name::<()>("set-remote-description", &[&description, &promise]);
    if recv_optional(&rx, HANDSHAKE) != Some(true) {
        return Err("set-remote-description failed".into());
    }
    Ok(())
}

fn session_description(kind: WebRTCSDPType, sdp: &str) -> Result<WebRTCSessionDescription, String> {
    let parsed = SDPMessage::parse_buffer(sdp.as_bytes())
        .map_err(|_| "SDP could not be parsed".to_string())?;
    Ok(WebRTCSessionDescription::new(kind, parsed))
}

fn sdp_text(webrtc: &gstreamer::Element, property: &str) -> Result<String, String> {
    let description = webrtc
        .property::<Option<WebRTCSessionDescription>>(property)
        .ok_or_else(|| format!("{property} is missing"))?;
    description
        .sdp()
        .as_text()
        .map_err(|_| format!("{property} could not be read"))
}

fn wait_ice(webrtc: &gstreamer::Element) -> Result<(), String> {
    if ice_complete(webrtc) {
        return Ok(());
    }
    let (tx, rx) = mpsc::channel();
    let _id = webrtc.connect_notify(Some("ice-gathering-state"), move |element, _| {
        if ice_complete(element) {
            let _ = tx.send(());
        }
    });
    if ice_complete(webrtc) {
        return Ok(());
    }
    recv_optional(&rx, HANDSHAKE).ok_or_else(|| "ICE gathering timed out".into())
}

fn ice_complete(webrtc: &gstreamer::Element) -> bool {
    webrtc.property::<WebRTCICEGatheringState>("ice-gathering-state")
        == WebRTCICEGatheringState::Complete
}

fn recv_result<T>(
    rx: &mpsc::Receiver<Result<T, String>>,
    timeout: Duration,
    step: &str,
) -> Result<T, String> {
    match recv_optional(rx, timeout) {
        Some(result) => result,
        None => Err(format!("{step} timed out")),
    }
}

fn recv_optional<T>(rx: &mpsc::Receiver<T>, timeout: Duration) -> Option<T> {
    let ctx = glib::MainContext::default();
    let start = Instant::now();
    loop {
        match rx.try_recv() {
            Ok(value) => return Some(value),
            Err(mpsc::TryRecvError::Disconnected) => return None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if start.elapsed() >= timeout {
            return None;
        }
        if !ctx.iteration(false) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn serve(pipeline: gstreamer::Pipeline, appsrc: AppSrc, jobs: Receiver<Job>) {
    let ctx = glib::MainContext::default();
    let period = Duration::from_nanos(1_000_000_000 / 30);
    let mut next = Instant::now();
    let mut last: Option<Vec<u8>> = None;
    loop {
        match jobs.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(Job::Frame(frame)) => last = Some(frame),
            Ok(Job::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = appsrc.end_of_stream();
                let _ = pipeline.set_state(gstreamer::State::Null);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let _ = ctx.iteration(false);
        let now = Instant::now();
        if now < next {
            continue;
        }
        next += period;
        if next <= now {
            next = now + period;
        }
        let Some(frame) = last.as_ref() else {
            continue;
        };
        push_frame(&appsrc, frame);
    }
}

fn push_frame(appsrc: &AppSrc, pixels: &[u8]) {
    let mut buffer = gstreamer::Buffer::from_mut_slice(pixels.to_vec());
    let _ = buffer
        .get_mut()
        .map(|buffer| buffer.set_pts(gstreamer::ClockTime::NONE));
    let _ = appsrc.push_buffer(buffer);
}

/// Build a webrtcbin answer for tests and loopback WHIP endpoints.
#[cfg(test)]
pub(crate) fn answer_sdp(offer: &str) -> Result<String, String> {
    init_gstreamer()?;
    if gstreamer::ElementFactory::find("webrtcbin").is_none() {
        return Err("webrtcbin is not installed".into());
    }
    let webrtc = gstreamer::ElementFactory::make("webrtcbin")
        .name("recv")
        .property_from_str("bundle-policy", "max-bundle")
        .build()
        .map_err(|err| format!("answer webrtcbin did not start: {err}"))?;
    let pipeline = gstreamer::Pipeline::default();
    pipeline
        .add(&webrtc)
        .map_err(|_| "answer webrtcbin could not be added".to_string())?;
    let pipeline_weak = pipeline.downgrade();
    webrtc.connect_pad_added(move |_, pad| {
        if pad.direction() != gstreamer::PadDirection::Src {
            return;
        }
        let Some(pipeline) = pipeline_weak.upgrade() else {
            return;
        };
        let Ok(sink) = gstreamer::ElementFactory::make("fakesink")
            .name("out")
            .property("async", false)
            .build()
        else {
            return;
        };
        if pipeline.add(&sink).is_err() {
            return;
        }
        let _ = sink.sync_state_with_parent();
        let Some(sink_pad) = sink.static_pad("sink") else {
            return;
        };
        let _ = pad.link(&sink_pad);
    });
    if pipeline.set_state(gstreamer::State::Ready).is_err() {
        let _ = pipeline.set_state(gstreamer::State::Null);
        return Err("answer webrtcbin did not become ready".into());
    }
    let offer_desc = session_description(WebRTCSDPType::Offer, offer)?;
    let (tx, rx) = mpsc::channel();
    let promise = gstreamer::Promise::with_change_func(move |reply| {
        let _ = tx.send(reply.is_ok());
    });
    webrtc.emit_by_name::<()>("set-remote-description", &[&offer_desc, &promise]);
    if recv_optional(&rx, HANDSHAKE) != Some(true) {
        let _ = pipeline.set_state(gstreamer::State::Null);
        return Err("answer set-remote-description failed".into());
    }
    let (tx, rx) = mpsc::channel();
    let promise = gstreamer::Promise::with_change_func(move |reply| {
        let result = match reply {
            Ok(Some(structure)) => structure
                .get::<WebRTCSessionDescription>("answer")
                .map_err(|err| format!("create-answer missing SDP: {err}")),
            Ok(None) => Err("create-answer returned no SDP".into()),
            Err(err) => Err(format!("create-answer failed: {err:?}")),
        };
        let _ = tx.send(result);
    });
    webrtc.emit_by_name::<()>("create-answer", &[&None::<gstreamer::Structure>, &promise]);
    let answer = match recv_result(&rx, HANDSHAKE, "create-answer") {
        Ok(answer) => answer,
        Err(err) => {
            let _ = pipeline.set_state(gstreamer::State::Null);
            return Err(err);
        }
    };
    let (tx, rx) = mpsc::channel();
    let promise = gstreamer::Promise::with_change_func(move |reply| {
        let _ = tx.send(reply.is_ok());
    });
    webrtc.emit_by_name::<()>("set-local-description", &[&answer, &promise]);
    if recv_optional(&rx, HANDSHAKE).is_none() {
        let _ = pipeline.set_state(gstreamer::State::Null);
        return Err("answer set-local-description timed out".into());
    }
    if let Err(err) = wait_ice(&webrtc) {
        let _ = pipeline.set_state(gstreamer::State::Null);
        return Err(err);
    }
    let sdp = sdp_text(&webrtc, "local-description");
    let _ = pipeline.set_state(gstreamer::State::Null);
    sdp
}

/// Create a local WHIP offer from the encode pipeline, used by tests.
#[cfg(test)]
pub(crate) fn whip_offer_sdp(width: u32, height: u32) -> Result<String, String> {
    let (pipeline, _appsrc, webrtc) = open_encode_pipeline(width, height, None)?;
    let result = local_offer(&webrtc);
    let _ = pipeline.set_state(gstreamer::State::Null);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn webrtcbin_builds_a_local_whip_offer() {
        let offer = whip_offer_sdp(64, 64).expect("local offer");
        assert!(offer.contains("v=0"), "offer should be SDP: {offer}");
        assert!(
            offer.to_ascii_uppercase().contains("H264") || offer.contains("m=video"),
            "offer should describe a video encode: {offer}"
        );
    }

    #[test]
    fn routed_audio_is_encoded_as_opus_rtp() {
        init_gstreamer().unwrap();
        let id = format!("whip-audio-{}", std::process::id());
        let pipeline = gstreamer::parse::launch(&format!(
            "audiotestsrc is-live=true volume=0.5 ! audio/x-raw,format=F32LE,rate=48000,channels=2 ! interaudiosink channel=\"{}\" sync=false {} ! rtpopusdepay ! opusdec ! audioconvert ! audio/x-raw,format=F32LE,channels=2 ! appsink name=capture sync=false",
            freeshow_media::network_channel(&id, false), audio_rtp(&id),
        ))
        .unwrap();
        let pipeline = pipeline.downcast::<gstreamer::Pipeline>().unwrap();
        let sink = pipeline
            .by_name("capture")
            .unwrap()
            .downcast::<gstreamer_app::AppSink>()
            .unwrap();
        pipeline.set_state(gstreamer::State::Playing).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut heard = false;
        while Instant::now() < deadline {
            if let Some(sample) = sink.try_pull_sample(gstreamer::ClockTime::from_mseconds(100)) {
                let map = sample.buffer().unwrap().map_readable().unwrap();
                heard = map.as_slice().as_chunks::<8>().0.iter().any(|frame| {
                    let [left, right] = frame.as_chunks::<4>().0 else {
                        unreachable!()
                    };
                    f32::from_le_bytes(*left).abs() > 0.1 && f32::from_le_bytes(*right).abs() > 0.1
                });
                if heard {
                    break;
                }
            }
        }
        pipeline.set_state(gstreamer::State::Null).unwrap();
        assert!(heard, "expected routed audio on both decoded Opus channels");
    }

    #[test]
    fn webrtcbin_posts_the_offer_and_applies_the_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/whip", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(8)))
                .unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|chunk| chunk == b"\r\n\r\n") && request.len() < 65536 {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap() == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            let header_end = request
                .windows(4)
                .position(|chunk| chunk == b"\r\n\r\n")
                .map(|index| index + 4)
                .unwrap_or(request.len());
            let headers = String::from_utf8_lossy(&request[..header_end]).to_string();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            let mut body = request[header_end..].to_vec();
            while body.len() < length {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap() == 0 {
                    break;
                }
                body.push(byte[0]);
            }
            let offer = String::from_utf8_lossy(&body).to_string();
            let answer = answer_sdp(&offer).expect("loopback answer");
            let response = format!(
                "HTTP/1.1 201 Created\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\nLocation: /whip/res\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            offer
        });
        let sender = WebrtcSender::open_routed(&url, Some("secret"), 64, 64, "audience")
            .expect("WHIP encode session");
        sender.submit(&[80u8; 64 * 64 * 4], 64, 64);
        drop(sender);
        let offer = server.join().unwrap();
        assert!(offer.contains("v=0"));
        assert!(
            offer.contains("m=audio "),
            "posted offer lacks audio: {offer}"
        );
        assert!(
            offer.to_ascii_lowercase().contains("opus/48000/2") && offer.contains("sprop-stereo=1"),
            "posted offer lacks stereo Opus: {offer}"
        );
        assert!(
            offer.to_ascii_uppercase().contains("H264") || offer.contains("m=video"),
            "posted offer should describe a video encode: {offer}"
        );
    }
}
