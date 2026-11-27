//! Client HTTP/1.1 minimo verso whisper-server su 127.0.0.1.
//!
//! Sincrono e senza runtime: `finish()` si chiama anche da un task tokio di
//! `actions.rs`, e il `reqwest` di Handy non ha né `blocking` né `multipart`.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

/// Una porta libera su 127.0.0.1, scelta dal sistema.
pub fn free_port() -> Result<u16> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("nessuna porta locale libera")?;
    Ok(l.local_addr()?.port())
}

fn connect(port: u16, timeout: Duration) -> Result<TcpStream> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let s = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(2)))?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    s.set_nodelay(true)?;
    Ok(s)
}

fn read_response(mut s: TcpStream) -> Result<(u16, Vec<u8>)> {
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("risposta HTTP senza intestazioni"))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let body = raw[split + 4..].to_vec();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| anyhow!("stato HTTP illeggibile"))?;
    let chunked = head
        .lines()
        .any(|l| l.to_ascii_lowercase().replace(' ', "") == "transfer-encoding:chunked");
    Ok((status, if chunked { dechunk(&body)? } else { body }))
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = b
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| anyhow!("chunk HTTP troncato"))?;
        let size_txt = String::from_utf8_lossy(&b[..eol]);
        let size = usize::from_str_radix(size_txt.split(';').next().unwrap_or("").trim(), 16)?;
        b = &b[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size {
            bail!("chunk HTTP troncato");
        }
        out.extend_from_slice(&b[..size]);
        b = &b[(size + 2).min(b.len())..];
    }
}

pub fn get(port: u16, path: &str, timeout: Duration) -> Result<(u16, Vec<u8>)> {
    let mut s = connect(port, timeout)?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )?;
    read_response(s)
}

/// POST multipart/form-data con campi di testo e un file.
pub fn post_multipart(
    port: u16,
    path: &str,
    fields: &[(&str, &str)],
    file_name: &str,
    file: &[u8],
    timeout: Duration,
) -> Result<(u16, Vec<u8>)> {
    let boundary = "----eaglevoiceasr7d1f0c";
    let mut body: Vec<u8> = Vec::with_capacity(file.len() + 1024);
    for (k, v) in fields {
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"
        )?;
    }
    write!(
        body,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: audio/wav\r\n\r\n"
    )?;
    body.extend_from_slice(file);
    write!(body, "\r\n--{boundary}--\r\n")?;

    let mut s = connect(port, timeout)?;
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )?;
    s.write_all(&body)?;
    read_response(s)
}

/// WAV PCM16 16 kHz mono in memoria.
pub fn wav_bytes(samples: &[f32]) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cur = std::io::Cursor::new(Vec::with_capacity(samples.len() * 2 + 44));
    {
        let mut w = hound::WavWriter::new(&mut cur, spec)?;
        for &x in samples {
            w.write_sample((x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        w.finalize()?;
    }
    Ok(cur.into_inner())
}
