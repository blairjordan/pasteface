//! File transcription providers. No remote request is made unless OpenAI is selected.
use crate::config::Config;
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, multipart};
use serde::Deserialize;
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const ENDPOINT: &str = "https://api.openai.com/v1/audio/transcriptions";
const MAX_AUDIO_BYTES: u64 = 25_000_000;

pub fn transcribe(config: &Config, audio: &Path, canceled: &AtomicBool) -> Result<String> {
    ensure!(!canceled.load(Ordering::Acquire), "Transcription canceled");
    let size = fs::metadata(audio)
        .context("Cannot read recorded audio")?
        .len();
    ensure!(
        size <= MAX_AUDIO_BYTES,
        "Recording exceeds OpenAI's 25 MB file limit. Use shorter chunks or the local provider."
    );
    let key = config.openai_api_key()?;
    let bytes = fs::read(audio).context("Cannot read recorded audio")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(request(
        ENDPOINT,
        &key,
        &config.openai_model,
        bytes,
        canceled,
    ))
}

async fn request(
    endpoint: &str,
    key: &str,
    model: &str,
    audio: Vec<u8>,
    canceled: &AtomicBool,
) -> Result<String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("Cannot initialize transcription connection")?;
    let operation = async {
        let file = multipart::Part::bytes(audio)
            .file_name("recording.wav")
            .mime_str("audio/wav")?;
        let form = multipart::Form::new()
            .text("model", model.to_owned())
            .text("response_format", "json")
            .part("file", file);
        let response = client
            .post(endpoint)
            .bearer_auth(key)
            .multipart(form)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    anyhow::anyhow!("OpenAI transcription timed out; recording is retained")
                } else {
                    anyhow::anyhow!("Could not connect to OpenAI; check your connection")
                }
            })?;
        let status = response.status();
        // Do not surface arbitrary response bodies: they may echo request data or secrets.
        if !status.is_success() {
            match status.as_u16() {
                401 => bail!("OpenAI rejected the API key. Update it in settings."),
                403 => bail!("OpenAI access denied. Check project and model permissions."),
                404 => bail!("OpenAI model unavailable. Check the model in settings."),
                413 => bail!("Recording exceeds OpenAI's file size limit. Use shorter chunks."),
                429 => bail!("OpenAI rate or billing limit reached. Check your API account."),
                500..=599 => bail!(
                    "OpenAI is temporarily unavailable (HTTP {status}). Recording is retained."
                ),
                _ => bail!(
                    "OpenAI transcription failed (HTTP {status}). Check model and audio settings."
                ),
            }
        }
        #[derive(Deserialize)]
        struct Transcript {
            text: String,
        }
        let result = response
            .json::<Transcript>()
            .await
            .map_err(|_| anyhow::anyhow!("OpenAI returned an invalid transcript"))?;
        Ok(result.text.trim().to_owned())
    };
    let cancellation = async {
        loop {
            if canceled.load(Ordering::Acquire) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        biased;
        _ = cancellation => bail!("Transcription canceled"),
        result = operation => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, mpsc},
        thread,
        time::Instant,
    };

    fn mock(status: &str, body: &str) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/v1/audio/transcriptions",
            listener.local_addr().unwrap()
        );
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = socket.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let size = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|n| n.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    if request.len() >= end + 4 + size {
                        break;
                    }
                }
            }
            tx.send(String::from_utf8_lossy(&request).to_string())
                .unwrap();
            socket.write_all(response.as_bytes()).unwrap();
        });
        (endpoint, rx, handle)
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    #[test]
    fn uploads_exact_model_and_returns_text() {
        let (url, received, handle) = mock("200 OK", r#"{"text":"  hello there  "}"#);
        let text = runtime()
            .block_on(request(
                &url,
                "mock-secret",
                "gpt-transcribe",
                b"fake-wav".to_vec(),
                &AtomicBool::new(false),
            ))
            .unwrap();
        assert_eq!(text, "hello there");
        let body = received.recv().unwrap();
        assert!(body.contains("name=\"model\"\r\n\r\ngpt-transcribe"));
        assert!(body.contains("filename=\"recording.wav\""));
        assert!(body.contains("audio/wav"));
        assert!(body.contains("Bearer mock-secret"));
        handle.join().unwrap();
    }
    #[test]
    fn server_errors_never_echo_response_secrets() {
        let (url, _received, handle) = mock("401 Unauthorized", r#"{"error":"mock-secret"}"#);
        let error = runtime()
            .block_on(request(
                &url,
                "mock-secret",
                "gpt-transcribe",
                vec![],
                &AtomicBool::new(false),
            ))
            .unwrap_err()
            .to_string();
        assert!(error.contains("API key"));
        assert!(!error.contains("mock-secret"));
        handle.join().unwrap();
    }
    #[test]
    fn cancellation_aborts_a_waiting_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let canceled = Arc::new(AtomicBool::new(false));
        let cancel = canceled.clone();
        let server = thread::spawn(move || {
            let (_socket, _) = listener.accept().unwrap();
            cancel.store(true, Ordering::Release);
            thread::sleep(Duration::from_millis(200));
        });
        let start = Instant::now();
        let result = runtime().block_on(request(
            &url,
            "mock-secret",
            "gpt-transcribe",
            vec![],
            &canceled,
        ));
        assert!(result.unwrap_err().to_string().contains("canceled"));
        assert!(start.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
}
