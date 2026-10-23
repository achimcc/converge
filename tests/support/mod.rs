//! A std-only HTTP server for tests: records every request and answers from a
//! fixed route table. One connection per request (`Connection: close`).

#![allow(dead_code)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

/// A route with this status takes the request and never answers: a service
/// that stands still (design §54).
pub const SILENT: u16 = 0;

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: String,
    pub body: String,
}

pub struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Recorded>>>,
}

impl Server {
    pub fn start(routes: Vec<(&'static str, &'static str, u16, String)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            // Connections of `SILENT` routes: kept open, never answered.
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let mut parts = head.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let length = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        if k.eq_ignore_ascii_case("content-length") {
                            v.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                log.lock().unwrap().push(Recorded {
                    method: method.clone(),
                    path: path.clone(),
                    headers: head.clone(),
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let (status, answer) = routes
                    .iter()
                    .find(|(m, p, _, _)| *m == method && *p == path)
                    .map(|(_, _, s, b)| (*s, b.clone()))
                    .unwrap_or((404, String::new()));
                if status == SILENT {
                    // The request was read in full and recorded; the client
                    // waits until its own time limit ends.
                    held.push(stream);
                    continue;
                }
                // An answer that starts with `Set-Cookie: ` carries header
                // lines first, up to an empty line, then the body.
                let (extra, answer) = match answer.strip_prefix("Set-Cookie: ") {
                    Some(_) => match answer.split_once("\r\n\r\n") {
                        Some((lines, body)) => (format!("{lines}\r\n"), body.to_string()),
                        None => (format!("{answer}\r\n"), String::new()),
                    },
                    None => (String::new(), answer),
                };
                // A 3xx route's answer is its `Location`.
                let _ = if (300..400).contains(&status) {
                    write!(
                        stream,
                        "HTTP/1.1 {status} X\r\nLocation: {answer}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    write!(
                        stream,
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                        answer.len()
                    )
                };
            }
        });
        Server { port, seen }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.seen.lock().unwrap().clone()
    }
}
