//! HTTP transport for the AI commands (Generative Fill, Remove Background) to an IOPaint server
//! (https://github.com/Sanster/IOPaint), normally on this PC or one on the local network.
//!
//! - `PHOTOCRAFT_AI_URL`: the server, default `http://127.0.0.1:8080`.
//! - `PHOTOCRAFT_AI_FILL_MODEL`: the model for prompted fills, default
//!   `Uminosachi/realisticVisionV51_v51VAE-inpainting`. Fills without a prompt use `lama`, which
//!   continues the surroundings (object removal), as Photoshop does for an empty prompt.
//!
//! The server loads one model at a time, so a fill switches it first when needed.

use std::time::Duration;

use base64::Engine as _;
use photocraft_engine::ai_cmds::{AiService, AiTask, AiTransport};
use serde_json::{Value, json};

const DEFAULT_URL: &str = "http://127.0.0.1:8080";
const DEFAULT_FILL_MODEL: &str = "Uminosachi/realisticVisionV51_v51VAE-inpainting";
const REMOVE_MODEL: &str = "lama";
/// Largest response accepted: a 4096² RGBA PNG is well under this.
const MAX_RESPONSE: u64 = 256 * 1024 * 1024;

pub fn service() -> AiService {
    let base = std::env::var("PHOTOCRAFT_AI_URL").ok().map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty());
    let base = base.unwrap_or_else(|| DEFAULT_URL.to_string());
    let fill_model = std::env::var("PHOTOCRAFT_AI_FILL_MODEL").ok().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| DEFAULT_FILL_MODEL.to_string());
    let endpoint = base.clone();
    let transport: AiTransport = std::sync::Arc::new(move |task: &AiTask| run(&base, &fill_model, task));
    AiService { transport: Some(transport), endpoint }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).max_redirects(0).build().new_agent()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// POST `body` as JSON and return the response bytes, with the server's own error text.
fn post(base: &str, path: &str, body: &Value, timeout: Duration) -> Result<Vec<u8>, String> {
    let url = format!("{base}{path}");
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let mut response =
        agent(timeout).post(&url).header("Content-Type", "application/json").header("User-Agent", "PhotoCraft-AI").send(&bytes[..]).map_err(|e| match e {
            ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
                format!("the AI server at {base} is not running. Start it (Start AI Server on the desktop) and try again")
            }
            ureq::Error::Timeout(_) => format!("the AI server at {base} took longer than {} s", timeout.as_secs()),
            e => format!("could not reach the AI server at {base}: {e}"),
        })?;
    let status = response.status().as_u16();
    let body = response.body_mut().with_config().limit(MAX_RESPONSE).read_to_vec().map_err(|e| format!("could not read the AI server's reply: {e}"))?;
    if status != 200 {
        let detail = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("detail").or_else(|| v.get("errors")).map(|d| d.as_str().map(str::to_string).unwrap_or_else(|| d.to_string())))
            .unwrap_or_else(|| String::from_utf8_lossy(&body).chars().take(300).collect());
        return Err(format!("the AI server answered {status}: {detail}"));
    }
    Ok(body)
}

/// Make `name` the server's model, unless it already is.
fn use_model(base: &str, name: &str) -> Result<(), String> {
    let current = agent(Duration::from_secs(10))
        .get(&format!("{base}/api/v1/model"))
        .call()
        .ok()
        .and_then(|mut r| r.body_mut().read_to_vec().ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string));
    if current.as_deref() == Some(name) {
        return Ok(());
    }
    // Loading a diffusion model from disk can take a few minutes on an older PC.
    post(base, "/api/v1/model", &json!({"name": name}), Duration::from_secs(900)).map(|_| ())
}

/// IOPaint's inpaint request. Without a `seed` the server picks a random one (`-1`), so each
/// fill differs, as in Photoshop; its own default is a fixed seed, which repeated every result.
fn inpaint_body(image: &[u8], mask: &[u8], prompt: &str, negative_prompt: &str, seed: Option<u64>) -> Value {
    json!({
        "image": b64(image),
        "mask": b64(mask),
        "prompt": prompt,
        "negative_prompt": negative_prompt,
        // Large areas are cut into the model's own size around the selection.
        "hd_strategy": "Crop",
        "hd_strategy_crop_trigger_size": 800,
        "hd_strategy_crop_margin": 128,
        // 25 steps: about a minute on a GTX 980, little visibly different from 30.
        "sd_steps": 25,
        "sd_guidance_scale": 7.5,
        "sd_mask_blur": 8,
        // Off, as in IOPaint's own UI: matching the fill's colour histogram to the image
        // flattened fills over plain backgrounds to a few colours.
        "sd_match_histograms": false,
        "sd_seed": seed.map_or(-1, |s| (s & u64::from(i32::MAX.unsigned_abs())) as i64),
    })
}

fn run(base: &str, fill_model: &str, task: &AiTask) -> Result<Vec<u8>, String> {
    match task {
        AiTask::Inpaint { image, mask, prompt, negative_prompt, seed } => {
            let model = if prompt.is_empty() { REMOVE_MODEL } else { fill_model };
            use_model(base, model)?;
            post(base, "/api/v1/inpaint", &inpaint_body(image, mask, prompt, negative_prompt, *seed), Duration::from_secs(900))
        }
        AiTask::RemoveBackground { image } => {
            post(base, "/api/v1/run_plugin_gen_image", &json!({"name": "RemoveBG", "image": b64(image)}), Duration::from_secs(300))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A one-request HTTP server: returns the request line and body it got.
    fn serve(status: &str, reply: &'static [u8]) -> (String, std::thread::JoinHandle<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let status = status.to_string();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let mut stream = stream;
            write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).unwrap();
            stream.write_all(reply).unwrap();
            (line, body)
        });
        (base, handle)
    }

    #[test]
    fn remove_background_posts_the_plugin_request() {
        let (base, server) = serve("200 OK", b"PNGDATA");
        let out = run(&base, DEFAULT_FILL_MODEL, &AiTask::RemoveBackground { image: vec![1, 2, 3] }).unwrap();
        assert_eq!(out, b"PNGDATA");
        let (line, body) = server.join().unwrap();
        assert!(line.starts_with("POST /api/v1/run_plugin_gen_image "), "{line}");
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["name"], "RemoveBG");
        assert_eq!(v["image"], "AQID");
    }

    #[test]
    fn fills_get_a_random_seed_unless_one_is_given() {
        assert_eq!(inpaint_body(&[], &[], "a barn", "", None)["sd_seed"], -1);
        assert_eq!(inpaint_body(&[], &[], "a barn", "", Some(7))["sd_seed"], 7);
        assert_eq!(inpaint_body(&[], &[], "a barn", "", Some(u64::MAX))["sd_seed"], i64::from(i32::MAX));
    }

    #[test]
    fn server_errors_carry_its_message() {
        let (base, server) = serve("422 Unprocessable Entity", br#"{"detail":"Plugin not found"}"#);
        let e = run(&base, DEFAULT_FILL_MODEL, &AiTask::RemoveBackground { image: vec![] }).unwrap_err();
        assert!(e.contains("422") && e.contains("Plugin not found"), "{e}");
        server.join().unwrap();
    }

    #[test]
    fn a_missing_server_says_so() {
        // Bind and drop: nothing listens on the port now.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let e = run(&format!("http://127.0.0.1:{port}"), DEFAULT_FILL_MODEL, &AiTask::RemoveBackground { image: vec![] }).unwrap_err();
        assert!(e.contains("not running"), "{e}");
    }
}
