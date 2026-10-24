//! HTTP transport for the AI commands (Generative Fill, Remove Background) to an IOPaint server
//! (https://github.com/Sanster/IOPaint), normally on this PC or one on the local network.
//!
//! - `PHOTOCRAFT_AI_URL`: the server, default `http://127.0.0.1:8080`.
//! - `PHOTOCRAFT_AI_FILL_MODEL`: the model for prompted fills, default
//!   `Uminosachi/realisticVisionV51_v51VAE-inpainting`. Fills without a prompt use `lama`, which
//!   continues the surroundings (object removal), as Photoshop does for an empty prompt.
//!
//! The server loads one model at a time, so a fill switches it first when needed. After a
//! prompted fill it is switched back to `lama` in the background, which frees most of the video
//! memory the diffusion model holds: on a 4 GB card shared with PhotoCraft's own canvas, keeping it
//! loaded left too little for the window, and the driver dropped PhotoCraft's GPU device.
//! `PHOTOCRAFT_AI_KEEP_FILL_MODEL=1` keeps it loaded (faster repeated fills on larger cards).
//!
//! - `PHOTOCRAFT_AI_CLOUD_URL`, `PHOTOCRAFT_AI_TOKEN`: a second server for prompted fills, such as
//!   the Kaggle notebook in `packaging/kaggle`, sent the token as `Authorization: Bearer`. Object
//!   removal and Remove Background stay on the local server, and use the cloud one only while the
//!   local one is not running. The cloud server keeps its fill model loaded.
//! - `PHOTOCRAFT_AI_CLOUD_FILE`: instead of those two, a file holding the URL on its first line and
//!   the token on its second, read for each request: a notebook's tunnel gets a new address each
//!   time it starts, and its launcher rewrites the file without PhotoCraft having to restart.

use std::time::Duration;

use base64::Engine as _;
use photocraft_engine::ai_cmds::{AiService, AiTask, AiTransport};
use serde_json::{Value, json};

const DEFAULT_URL: &str = "http://127.0.0.1:8080";
const DEFAULT_FILL_MODEL: &str = "Uminosachi/realisticVisionV51_v51VAE-inpainting";
const REMOVE_MODEL: &str = "lama";
/// Largest response accepted: a 4096² RGBA PNG is well under this.
const MAX_RESPONSE: u64 = 256 * 1024 * 1024;

/// One IOPaint server: its address and, for one reached over the internet, the token it wants.
#[derive(Clone, Debug, PartialEq)]
struct Server {
    base: String,
    token: Option<String>,
}

fn env_url(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty())
}

/// Where the cloud server's address comes from.
enum Cloud {
    Fixed(Server),
    File(std::path::PathBuf),
}

impl Cloud {
    fn server(&self) -> Result<Server, Failure> {
        match self {
            Cloud::Fixed(server) => Ok(server.clone()),
            Cloud::File(path) => {
                let text = std::fs::read_to_string(path).unwrap_or_default();
                let mut lines = text.lines().map(str::trim);
                let base = lines.next().unwrap_or_default().trim_end_matches('/').to_string();
                let token = lines.next().filter(|t| !t.is_empty()).map(str::to_string);
                if base.is_empty() {
                    return Err(Failure {
                        unreachable: true,
                        message: "the cloud AI server is not running. Start it (Start Cloud AI on the desktop) and try again".into(),
                    });
                }
                Ok(Server { base, token })
            }
        }
    }
}

/// Which server each task goes to.
struct Router {
    local: Server,
    cloud: Option<Cloud>,
    fill_model: String,
    keep_fill_model: bool,
}

impl Router {
    fn from_env() -> Self {
        let local = Server { base: env_url("PHOTOCRAFT_AI_URL").unwrap_or_else(|| DEFAULT_URL.to_string()), token: None };
        let token = std::env::var("PHOTOCRAFT_AI_TOKEN").ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let cloud = match env_url("PHOTOCRAFT_AI_CLOUD_FILE") {
            Some(path) => Some(Cloud::File(path.into())),
            None => env_url("PHOTOCRAFT_AI_CLOUD_URL").map(|base| Cloud::Fixed(Server { base, token })),
        };
        let fill_model = std::env::var("PHOTOCRAFT_AI_FILL_MODEL").ok().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| DEFAULT_FILL_MODEL.to_string());
        let keep_fill_model = std::env::var("PHOTOCRAFT_AI_KEEP_FILL_MODEL").is_ok_and(|v| v == "1");
        Router { local, cloud, fill_model, keep_fill_model }
    }

    fn call(&self, task: &AiTask) -> Result<Vec<u8>, String> {
        let prompted = matches!(task, AiTask::Inpaint { prompt, .. } if !prompt.is_empty());
        let on_cloud = |cloud: &Cloud| cloud.server().and_then(|server| run(&server, &self.fill_model, task));
        let (result, ran_locally) = match &self.cloud {
            Some(cloud) if prompted => (on_cloud(cloud), false),
            Some(cloud) => match run(&self.local, &self.fill_model, task) {
                Err(e) if e.unreachable => match on_cloud(cloud) {
                    // Neither runs: the local server is the one to start for this task.
                    Err(c) if c.unreachable => (Err(e), true),
                    r => (r, false),
                },
                r => (r, true),
            },
            None => (run(&self.local, &self.fill_model, task), true),
        };
        if prompted && !self.keep_fill_model && ran_locally {
            let local = self.local.clone();
            // Errors only mean the next object removal switches instead.
            let _ = std::thread::Builder::new().name("AI model unload".into()).spawn(move || use_model(&local, REMOVE_MODEL));
        }
        result.map_err(|e| e.message)
    }
}

pub fn service() -> AiService {
    let router = Router::from_env();
    let endpoint = match &router.cloud {
        Some(_) => format!("{}, prompted fills on the cloud server", router.local.base),
        None => router.local.base.clone(),
    };
    let transport: AiTransport = std::sync::Arc::new(move |task: &AiTask| router.call(task));
    AiService { transport: Some(transport), endpoint }
}

/// Why a request failed; `unreachable` when nothing answered at the address.
#[derive(Debug)]
struct Failure {
    unreachable: bool,
    message: String,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure { unreachable: false, message }
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).max_redirects(0).build().new_agent()
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn not_running(server: &Server) -> Failure {
    let base = &server.base;
    let message = if server.token.is_some() {
        format!("the cloud AI server at {base} is not running. Start it (Start Cloud AI on the desktop) and try again")
    } else {
        format!("the AI server at {base} is not running. Start it (Start AI Server on the desktop) and try again")
    };
    Failure { unreachable: true, message }
}

/// POST `body` as JSON and return the response bytes, with the server's own error text.
fn post(server: &Server, path: &str, body: &Value, timeout: Duration) -> Result<Vec<u8>, Failure> {
    let base = &server.base;
    let url = format!("{base}{path}");
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let mut request = agent(timeout).post(&url).header("Content-Type", "application/json").header("User-Agent", "PhotoCraft-AI");
    if let Some(token) = &server.token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.send(&bytes[..]).map_err(|e| match e {
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => not_running(server),
        ureq::Error::Timeout(_) => format!("the AI server at {base} took longer than {} s", timeout.as_secs()).into(),
        e => format!("could not reach the AI server at {base}: {e}").into(),
    })?;
    let status = response.status().as_u16();
    let body = response.body_mut().with_config().limit(MAX_RESPONSE).read_to_vec().map_err(|e| format!("could not read the AI server's reply: {e}"))?;
    // A tunnel whose notebook has stopped answers 502-530 from the tunnel's own edge.
    if server.token.is_some() && (502..=530).contains(&status) {
        return Err(not_running(server));
    }
    if status != 200 {
        let detail = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("detail").or_else(|| v.get("errors")).map(|d| d.as_str().map(str::to_string).unwrap_or_else(|| d.to_string())))
            .unwrap_or_else(|| String::from_utf8_lossy(&body).chars().take(300).collect());
        return Err(format!("the AI server answered {status}: {detail}").into());
    }
    Ok(body)
}

/// Make `name` the server's model, unless it already is.
fn use_model(server: &Server, name: &str) -> Result<(), Failure> {
    let mut request = agent(Duration::from_secs(10)).get(&format!("{}/api/v1/model", server.base));
    if let Some(token) = &server.token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let current = request
        .call()
        .ok()
        .filter(|r| r.status() == 200)
        .and_then(|mut r| r.body_mut().read_to_vec().ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string));
    if current.as_deref() == Some(name) {
        return Ok(());
    }
    // Loading a diffusion model from disk can take a few minutes on an older PC.
    post(server, "/api/v1/model", &json!({"name": name}), Duration::from_secs(900)).map(|_| ())
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

fn run(server: &Server, fill_model: &str, task: &AiTask) -> Result<Vec<u8>, Failure> {
    match task {
        AiTask::Inpaint { image, mask, prompt, negative_prompt, seed } => {
            let model = if prompt.is_empty() { REMOVE_MODEL } else { fill_model };
            use_model(server, model)?;
            post(server, "/api/v1/inpaint", &inpaint_body(image, mask, prompt, negative_prompt, *seed), Duration::from_secs(900))
        }
        AiTask::RemoveBackground { image } => {
            post(server, "/api/v1/run_plugin_gen_image", &json!({"name": "RemoveBG", "image": b64(image)}), Duration::from_secs(300))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    struct Got {
        line: String,
        headers: String,
        body: Vec<u8>,
    }

    /// A one-request HTTP server: returns the request line, headers (lowercased) and body it got.
    fn serve(status: &str, reply: &'static [u8]) -> (String, std::thread::JoinHandle<Got>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let status = status.to_string();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut headers = String::new();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                let h = h.to_ascii_lowercase();
                if let Some(v) = h.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                headers.push_str(&h);
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let mut stream = stream;
            write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).unwrap();
            stream.write_all(reply).unwrap();
            Got { line, headers, body }
        });
        (base, handle)
    }

    fn local(base: &str) -> Server {
        Server { base: base.to_string(), token: None }
    }

    /// An address nothing listens on: bound, then dropped.
    fn nothing() -> String {
        format!("http://127.0.0.1:{}", TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port())
    }

    fn router(local_base: &str, cloud: Option<(&str, &str)>) -> Router {
        Router {
            local: self::local(local_base),
            cloud: cloud.map(|(base, token)| Cloud::Fixed(Server { base: base.to_string(), token: Some(token.to_string()) })),
            fill_model: DEFAULT_FILL_MODEL.to_string(),
            keep_fill_model: true,
        }
    }

    #[test]
    fn remove_background_posts_the_plugin_request() {
        let (base, server) = serve("200 OK", b"PNGDATA");
        let out = run(&local(&base), DEFAULT_FILL_MODEL, &AiTask::RemoveBackground { image: vec![1, 2, 3] }).unwrap();
        assert_eq!(out, b"PNGDATA");
        let got = server.join().unwrap();
        assert!(got.line.starts_with("POST /api/v1/run_plugin_gen_image "), "{}", got.line);
        assert!(!got.headers.contains("authorization"), "{}", got.headers);
        let v: Value = serde_json::from_slice(&got.body).unwrap();
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
        let e = run(&local(&base), DEFAULT_FILL_MODEL, &AiTask::RemoveBackground { image: vec![] }).unwrap_err();
        assert!(!e.unreachable && e.message.contains("422") && e.message.contains("Plugin not found"), "{e:?}");
        server.join().unwrap();
    }

    #[test]
    fn a_missing_server_says_so() {
        let e = router(&nothing(), None).call(&AiTask::RemoveBackground { image: vec![] }).unwrap_err();
        assert!(e.contains("not running") && e.contains("Start AI Server"), "{e}");
    }

    #[test]
    fn the_cloud_server_gets_the_token() {
        let (base, server) = serve("200 OK", b"PNGDATA");
        let cloud = Server { base, token: Some("s3cret".into()) };
        run(&cloud, DEFAULT_FILL_MODEL, &AiTask::RemoveBackground { image: vec![] }).unwrap();
        let got = server.join().unwrap();
        assert!(got.headers.contains("authorization: bearer s3cret\r\n"), "{}", got.headers);
    }

    #[test]
    fn without_the_local_server_the_cloud_one_is_used() {
        let (cloud, server) = serve("200 OK", b"PNGDATA");
        let out = router(&nothing(), Some((&cloud, "t"))).call(&AiTask::RemoveBackground { image: vec![9] }).unwrap();
        assert_eq!(out, b"PNGDATA");
        assert!(server.join().unwrap().line.starts_with("POST /api/v1/run_plugin_gen_image "));
    }

    #[test]
    fn a_local_server_error_is_not_retried_on_the_cloud() {
        let (base, server) = serve("500 Internal Server Error", br#"{"detail":"out of memory"}"#);
        let e = router(&base, Some((&nothing(), "t"))).call(&AiTask::RemoveBackground { image: vec![] }).unwrap_err();
        assert!(e.contains("out of memory"), "{e}");
        server.join().unwrap();
    }

    #[test]
    fn the_cloud_address_is_read_from_its_file_for_each_request() {
        let dir = std::env::temp_dir().join(format!("photocraft-ai-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cloud.txt");
        let mut r = router(&nothing(), None);
        r.cloud = Some(Cloud::File(file.clone()));
        let task = AiTask::Inpaint { image: vec![], mask: vec![], prompt: "a barn".into(), negative_prompt: String::new(), seed: None };
        let e = r.call(&task).unwrap_err();
        assert!(e.contains("Start Cloud AI"), "{e}");
        let (base, server) = serve("200 OK", b"PNGDATA");
        std::fs::write(&file, format!("{base}/\r\ntok\r\n")).unwrap();
        let out = r.call(&AiTask::RemoveBackground { image: vec![] }).unwrap();
        assert_eq!(out, b"PNGDATA");
        assert!(server.join().unwrap().headers.contains("authorization: bearer tok\r\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn with_neither_server_running_the_local_one_is_named() {
        let e = router(&nothing(), Some((&nothing(), "t"))).call(&AiTask::RemoveBackground { image: vec![] }).unwrap_err();
        assert!(e.contains("Start AI Server"), "{e}");
    }

    #[test]
    fn a_stopped_tunnel_counts_as_a_stopped_server() {
        let (base, server) = serve("530 Origin Unregistered", b"<html>error 1033</html>");
        let cloud = Server { base, token: Some("t".into()) };
        let e = run(&cloud, DEFAULT_FILL_MODEL, &AiTask::RemoveBackground { image: vec![] }).unwrap_err();
        assert!(e.unreachable && e.message.contains("Start Cloud AI"), "{e:?}");
        server.join().unwrap();
    }

    #[test]
    fn a_stopped_cloud_server_says_how_to_start_it() {
        let task = AiTask::Inpaint { image: vec![], mask: vec![], prompt: "a barn".into(), negative_prompt: String::new(), seed: None };
        let e = router(&nothing(), Some((&nothing(), "t"))).call(&task).unwrap_err();
        assert!(e.contains("Start Cloud AI"), "{e}");
    }
}
