// SPDX-License-Identifier: GPL-3.0-only

use std::time::Duration;

/// POST an SDP offer to a WHIP endpoint and return the answer plus Location.
/// The show keeps running when the URL is missing; this call is used by a
/// WebRTC sender once GStreamer webrtcbin can be linked on the host.
pub fn whip_post(
    url: &str,
    token: Option<&str>,
    offer: &str,
) -> Result<(String, Option<String>), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("WHIP URL must use HTTP or HTTPS".into());
    }
    let mut request = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(15))
        .timeout(Duration::from_secs(20))
        .build()
        .post(url)
        .set("Content-Type", "application/sdp")
        .set("User-Agent", "OBS/30.0.0");
    if let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let response = request.send_string(offer).map_err(|err| match err {
        ureq::Error::Status(code, _) => format!("WHIP endpoint returned HTTP {code}"),
        ureq::Error::Transport(_) => "WHIP offer failed".into(),
    })?;
    if response.status() != 200 && response.status() != 201 {
        return Err(format!(
            "WHIP endpoint returned unexpected HTTP {}",
            response.status()
        ));
    }
    let location = response
        .header("Location")
        .or_else(|| response.header("location"))
        .map(str::to_string);
    let answer = response
        .into_string()
        .map_err(|_| "WHIP answer was not text")?;
    if answer.trim().is_empty() {
        return Err("WHIP endpoint returned an empty SDP answer".into());
    }
    Ok((answer, location))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn serve_once(
        status_line: &str,
        location: Option<&str>,
        body: &str,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/whip", listener.local_addr().unwrap());
        let header = if let Some(location) = location {
            format!(
                "{status_line}\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\nLocation: {location}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        } else {
            format!(
                "{status_line}\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        };
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
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
            stream.write_all(header.as_bytes()).unwrap();
            format!("{}{}", headers, String::from_utf8_lossy(&body))
        });
        (url, thread)
    }

    #[test]
    fn whip_post_sends_the_offer_token_and_reads_the_answer() {
        let (url, server) = serve_once(
            "HTTP/1.1 201 Created",
            Some("/whip/res"),
            "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n",
        );
        let (answer, location) = whip_post(&url, Some("secret"), "v=0\r\noffer").unwrap();
        let request = server.join().unwrap();
        assert!(answer.starts_with("v=0"));
        assert_eq!(location.as_deref(), Some("/whip/res"));
        assert!(request.starts_with("POST /whip "));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("content-type: application/sdp")
        );
        assert!(request.contains("Authorization: Bearer secret"));
        assert!(request.contains("v=0\r\noffer"));
    }
}
