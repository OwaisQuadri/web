use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

const LOOPBACK_ADDRESS: Ipv4Addr = Ipv4Addr::LOCALHOST;

struct Resource {
    content_type: &'static str,
    body: &'static [u8],
    is_audio: bool,
}

fn resource_for_path(target: &str) -> Option<Resource> {
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    if !path.starts_with('/') || path.contains("..") || path.contains('\\') {
        return None;
    }

    match path {
        "/smoke.html" => Some(Resource {
            content_type: "text/html; charset=utf-8",
            body: include_bytes!("../../fixtures/smoke.html"),
            is_audio: false,
        }),
        "/cookie.html" => Some(Resource {
            content_type: "text/html; charset=utf-8",
            body: include_bytes!("../../fixtures/cookie.html"),
            is_audio: false,
        }),
        "/frame.html" => Some(Resource {
            content_type: "text/html; charset=utf-8",
            body: include_bytes!("../../fixtures/frame.html"),
            is_audio: false,
        }),
        "/worker.js" => Some(Resource {
            content_type: "text/javascript; charset=utf-8",
            body: include_bytes!("../../fixtures/worker.js"),
            is_audio: false,
        }),
        "/audio.html" => Some(Resource {
            content_type: "text/html; charset=utf-8",
            body: include_bytes!("../../fixtures/audio.html"),
            is_audio: true,
        }),
        "/audio-frame.html" => Some(Resource {
            content_type: "text/html; charset=utf-8",
            body: include_bytes!("../../fixtures/audio-frame.html"),
            is_audio: true,
        }),
        "/article.html" => Some(Resource {
            content_type: "text/html; charset=utf-8",
            body: include_bytes!("../../fixtures/article.html"),
            is_audio: false,
        }),
        "/images.html" => Some(Resource {
            content_type: "text/html; charset=utf-8",
            body: include_bytes!("../../fixtures/images.html"),
            is_audio: false,
        }),
        "/application.html" => Some(Resource {
            content_type: "text/html; charset=utf-8",
            body: include_bytes!("../../fixtures/application.html"),
            is_audio: false,
        }),
        "/benchmark.js" => Some(Resource {
            content_type: "text/javascript; charset=utf-8",
            body: include_bytes!("../../fixtures/benchmark.js"),
            is_audio: false,
        }),
        "/benchmark.css" => Some(Resource {
            content_type: "text/css; charset=utf-8",
            body: include_bytes!("../../fixtures/benchmark.css"),
            is_audio: false,
        }),
        "/benchmark-red.png" => Some(Resource {
            content_type: "image/png",
            body: include_bytes!("../../fixtures/benchmark-red.png"),
            is_audio: false,
        }),
        "/benchmark-green.png" => Some(Resource {
            content_type: "image/png",
            body: include_bytes!("../../fixtures/benchmark-green.png"),
            is_audio: false,
        }),
        "/benchmark-blue.png" => Some(Resource {
            content_type: "image/png",
            body: include_bytes!("../../fixtures/benchmark-blue.png"),
            is_audio: false,
        }),
        _ => None,
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    resource: Option<Resource>,
    port: u16,
) -> io::Result<()> {
    let (content_type, body, policy) = resource.map_or(
        (
            "text/plain; charset=utf-8",
            b"not found".as_slice(),
            "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; media-src 'self'".to_owned(),
        ),
        |resource| {
            let policy = if resource.is_audio {
                format!("default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'none'; media-src 'self' blob:; frame-src http://localhost:{port}; frame-ancestors http://127.0.0.1:{port}")
            } else {
                "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; media-src 'self'".to_owned()
            };
            (resource.content_type, resource.body, policy)
        },
    );
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Security-Policy: {policy}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

fn read_request(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let start = Instant::now();
    let mut request = Vec::new();
    let mut chunk = [0_u8; 512];
    loop {
        if start.elapsed() > Duration::from_secs(2) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "request deadline exceeded",
            ));
        }
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete request",
            ));
        }
        request.extend_from_slice(&chunk[..count]);
        if request.len() > 8192 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request exceeds limit",
            ));
        }
        if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            return Ok(request);
        }
    }
}

fn resource_from_request(request: &[u8]) -> Option<Resource> {
    let request = std::str::from_utf8(request).ok()?;
    let mut parts = request.lines().next()?.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if method != "GET" || !matches!(version, "HTTP/1.0" | "HTTP/1.1") || parts.next().is_some() {
        return None;
    }
    resource_for_path(target)
}

fn handle_connection(stream: &mut TcpStream, port: u16) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let request = read_request(stream)?;
    let resource = resource_from_request(&request);
    let status = if resource.is_some() {
        "200 OK"
    } else {
        "404 Not Found"
    };
    write_response(stream, status, resource, port)
}

/// Serves embedded fixtures using command-line `arguments`; returns an error for invalid ports or listener failures.
pub fn run(arguments: impl Iterator<Item = String>) -> io::Result<()> {
    let mut arguments = arguments.skip(1);
    let port = match (arguments.next().as_deref(), arguments.next()) {
        (Some("--port"), Some(port)) if arguments.next().is_none() => {
            port.parse::<u16>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid port: {error}"),
                )
            })?
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: fixture-server --port <port>",
            ));
        }
    };
    let listener = TcpListener::bind((LOOPBACK_ADDRESS, port))?;
    println!("http://localhost:{}", listener.local_addr()?.port());
    io::stdout().flush()?;
    serve_connections(listener.incoming())
}

fn serve_connections(connections: impl Iterator<Item = io::Result<TcpStream>>) -> io::Result<()> {
    let (sender, receiver) = std::sync::mpsc::sync_channel::<TcpStream>(8);
    let receiver = std::sync::Mutex::new(receiver);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let receiver = &receiver;
            scope.spawn(move || {
                loop {
                    let request = receiver.lock().expect("fixture queue lock poisoned").recv();
                    let Ok(mut stream) = request else {
                        break;
                    };
                    let port = stream.local_addr().map_or(0, |address| address.port());
                    if let Err(error) = handle_connection(&mut stream, port) {
                        eprintln!("fixture request failed: {error}");
                    }
                }
            });
        }
        for stream in connections {
            sender
                .send(stream?)
                .map_err(|_| io::Error::other("fixture workers stopped"))?;
        }
        drop(sender);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{read_request, resource_for_path, resource_from_request, serve_connections};
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::time::Duration;

    #[test]
    fn serves_a_request_while_another_connection_is_idle() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || serve_connections(listener.incoming().take(2)));
        let idle = TcpStream::connect(address).unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client
            .write_all(b"GET /smoke.html HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        let result = client.read_to_string(&mut response);
        drop(idle);
        server.join().unwrap().unwrap();
        assert!(
            result.is_ok(),
            "request stalled behind idle connection: {result:?}"
        );
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Engine smoke"));
    }

    #[test]
    fn audio_headers_use_the_bound_port_not_the_host_header() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || serve_connections(listener.incoming().take(1)));
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client
            .write_all(b"GET /audio.html HTTP/1.1\r\nHost: unrelated.invalid:9\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        server.join().unwrap().unwrap();
        let headers = response.split("\r\n\r\n").next().unwrap();
        assert!(headers.contains(&format!("frame-src http://localhost:{};", address.port())));
        assert!(headers.contains(&format!(
            "frame-ancestors http://127.0.0.1:{}",
            address.port()
        )));
        assert!(!headers.contains("unrelated.invalid"));
        assert!(headers.contains("connect-src 'none'"));
    }

    #[test]
    fn ordinary_fixtures_allow_only_local_styles_and_content() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || serve_connections(listener.incoming().take(1)));
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client
            .write_all(b"GET /smoke.html HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        server.join().unwrap().unwrap();
        let policy = response
            .lines()
            .find(|line| line.starts_with("Content-Security-Policy:"))
            .unwrap();
        assert_eq!(
            policy,
            "Content-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; media-src 'self'"
        );
    }

    #[test]
    fn reads_a_fragmented_request() {
        let mut request = b"GET /smo"
            .as_slice()
            .chain(b"ke.html HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice());
        let bytes = read_request(&mut request).unwrap();
        assert!(resource_from_request(&bytes).is_some());
    }

    #[test]
    fn rejects_incomplete_and_oversized_requests() {
        assert!(read_request(&mut b"GET /smoke.html".as_slice()).is_err());
        assert!(read_request(&mut vec![b'A'; 8193].as_slice()).is_err());
    }

    #[test]
    fn rejects_malformed_request_lines() {
        for request in [
            "GET /smoke.html INVALID\r\n\r\n",
            "POST /smoke.html HTTP/1.1\r\n\r\n",
            "GET /smoke.html HTTP/1.1 extra\r\n\r\n",
        ] {
            assert!(resource_from_request(request.as_bytes()).is_none());
        }
    }

    #[test]
    fn serves_only_named_embedded_resources() {
        assert!(resource_for_path("/smoke.html?value=alpha").is_some());
        let cookie = resource_for_path("/cookie.html").unwrap();
        assert!(cookie.body.windows(9).any(|window| window == b"id=\"ready"));
        assert!(
            !cookie
                .body
                .windows(12)
                .any(|window| window == b"localStorage")
        );
        assert!(!cookie.body.windows(9).any(|window| window == b"indexedDB"));
        assert!(resource_for_path("/frame.html").is_some());
        assert!(resource_for_path("/worker.js").is_some());
        assert!(resource_for_path("/audio.html").is_some());
        assert!(resource_for_path("/audio-frame.html").is_some());
        for path in [
            "/article.html",
            "/images.html",
            "/application.html",
            "/benchmark.js",
            "/benchmark.css",
            "/benchmark-red.png",
            "/benchmark-green.png",
            "/benchmark-blue.png",
        ] {
            assert!(resource_for_path(path).is_some(), "missing {path}");
        }
    }

    #[test]
    fn benchmark_fixtures_have_fixed_content_contracts() {
        let script = resource_for_path("/benchmark.js").unwrap();
        let script = std::str::from_utf8(script.body).unwrap();
        assert!(script.contains("index <= 100"));
        assert!(script.contains("index <= 2000"));
        assert!(script.contains("image.decode()"));
        assert!(script.contains("state.details = details"));
        assert!(script.contains("webBenchmarkStatus"));
        assert!(script.contains("webBenchmarkAct"));

        for path in [
            "/benchmark-red.png",
            "/benchmark-green.png",
            "/benchmark-blue.png",
        ] {
            let image = resource_for_path(path).unwrap();
            assert_eq!(image.content_type, "image/png");
            assert!(image.body.starts_with(b"\x89PNG\r\n\x1a\n"));
            assert_eq!(
                u32::from_be_bytes(image.body[16..20].try_into().unwrap()),
                1024
            );
            assert_eq!(
                u32::from_be_bytes(image.body[20..24].try_into().unwrap()),
                768
            );
        }
    }

    #[test]
    fn rejects_traversal_and_unknown_resources() {
        assert!(resource_for_path("/../Cargo.toml").is_none());
        assert!(resource_for_path("/%2e%2e/Cargo.toml").is_none());
        assert!(resource_for_path("/unknown.html").is_none());
        assert!(resource_for_path("smoke.html").is_none());
    }
}
