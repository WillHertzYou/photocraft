"""PhotoCraft's cloud AI server, run as a Kaggle notebook on its free GPU.

Starts IOPaint (https://github.com/Sanster/IOPaint) with the Generative Fill model, behind a small
proxy that only answers requests carrying the token, and a Cloudflare quick tunnel that gives it a
public https address. When the server is ready, the address is posted to an ntfy.sh topic that
`start-cloud-ai.ps1` on the PC waits on. The notebook stops itself after IDLE_MINUTES without a
request, or when PhotoCraft's launcher asks it to (`stop-cloud-ai.ps1`), to save GPU quota.

`start-cloud-ai.ps1` fills in TOKEN, TOPIC and IDLE_MINUTES before pushing this file; the notebook
is private.
"""

import hmac
import http.client
import http.server
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.request

TOKEN = "__PHOTOCRAFT_TOKEN__"
TOPIC = "__PHOTOCRAFT_TOPIC__"
IDLE_MINUTES = 30
FILL_MODEL = "Uminosachi/realisticVisionV51_v51VAE-inpainting"

WORK = "/tmp/photocraft-ai"
VENV = f"{WORK}/venv"
MODELS = f"{WORK}/models"
IOPAINT_PORT = 8081
PROXY_PORT = 8000

last_request = time.monotonic()
stop = threading.Event()


def log(*args):
    print(time.strftime("%H:%M:%S"), *args, flush=True)


def notify(message):
    try:
        urllib.request.urlopen(urllib.request.Request(f"https://ntfy.sh/{TOPIC}", data=message.encode(), method="POST"), timeout=20)
    except Exception as e:  # The PC then times out waiting; the log says why.
        log("could not post to ntfy:", e)


def sh(*cmd, **kw):
    log("$", " ".join(cmd))
    subprocess.run(cmd, check=True, **kw)


def install():
    """IOPaint 1.6 pins old packages (Pillow 9.5, diffusers 0.27), so it gets its own Python 3.11
    instead of Kaggle's. PyTorch's CUDA 12.6 builds still cover the P100 (sm_60) as well as the T4."""
    os.makedirs(WORK, exist_ok=True)
    sh(sys.executable, "-m", "pip", "install", "-q", "uv")
    sh("uv", "venv", "-q", "--python", "3.11", VENV)
    py = f"{VENV}/bin/python"
    sh("uv", "pip", "install", "-q", "--python", py, "torch==2.7.1", "torchvision==0.22.1", "--index-url", "https://download.pytorch.org/whl/cu126")
    sh("uv", "pip", "install", "-q", "--python", py, "iopaint==1.6.0")
    # Remove Background needs rembg; without its own dependencies, so IOPaint's pins stay.
    sh("uv", "pip", "install", "-q", "--python", py, "--no-deps", "rembg==2.0.59")
    sh("uv", "pip", "install", "-q", "--python", py, "onnxruntime", "pooch", "pymatting", "numpy==1.26.4", "pillow==9.5.0")
    cloudflared = f"{WORK}/cloudflared"
    if not os.path.exists(cloudflared):
        url = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64"
        with urllib.request.urlopen(url, timeout=120) as r, open(cloudflared, "wb") as f:
            shutil.copyfileobj(r, f)
        os.chmod(cloudflared, 0o755)
    return cloudflared


def start_iopaint():
    env = dict(os.environ, HF_HOME=f"{MODELS}/huggingface", TORCH_HOME=f"{MODELS}/torch", HF_HUB_DISABLE_TELEMETRY="1")
    cmd = [f"{VENV}/bin/iopaint", "start", "--host", "127.0.0.1", "--port", str(IOPAINT_PORT),
           "--model", FILL_MODEL, "--device", "cuda", "--model-dir", MODELS,
           "--enable-remove-bg", "--remove-bg-model", "briaai/RMBG-1.4", "--remove-bg-device", "cuda"]
    log("$", " ".join(cmd))
    return subprocess.Popen(cmd, env=env)


def iopaint_ready():
    try:
        c = http.client.HTTPConnection("127.0.0.1", IOPAINT_PORT, timeout=5)
        c.request("GET", "/api/v1/server-config")
        return c.getresponse().status == 200
    except OSError:
        return False


class Proxy(http.server.BaseHTTPRequestHandler):
    """Forwards /api/v1/* to IOPaint for requests with the token; refuses everything else."""

    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        log("proxy:", fmt % args)

    def reply(self, status, body, content_type="application/json"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def authorized(self):
        given = self.headers.get("Authorization", "")
        return hmac.compare_digest(given.encode(), f"Bearer {TOKEN}".encode())

    def handle_any(self):
        global last_request
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        if not self.authorized():
            return self.reply(401, b'{"detail":"PhotoCraft cloud AI: wrong or missing token"}')
        last_request = time.monotonic()
        if self.path == "/photocraft/stop":
            self.reply(200, b'{"stopping":true}')
            stop.set()
            return
        if not self.path.startswith("/api/v1/"):
            return self.reply(404, b'{"detail":"not found"}')
        try:
            c = http.client.HTTPConnection("127.0.0.1", IOPAINT_PORT, timeout=900)
            headers = {"Content-Type": self.headers.get("Content-Type", "application/json")}
            c.request(self.command, self.path, body=body or None, headers=headers)
            r = c.getresponse()
            data = r.read()
            self.reply(r.status, data, r.getheader("Content-Type", "application/octet-stream"))
        except OSError as e:
            self.reply(502, json.dumps({"detail": f"IOPaint is not answering: {e}"}).encode())
        last_request = time.monotonic()

    do_GET = do_POST = handle_any


def start_tunnel(cloudflared):
    p = subprocess.Popen([cloudflared, "tunnel", "--no-autoupdate", "--url", f"http://127.0.0.1:{PROXY_PORT}"],
                         stderr=subprocess.PIPE, text=True)
    url = None
    deadline = time.monotonic() + 120
    for line in p.stderr:
        print("cloudflared:", line.rstrip(), flush=True)
        m = re.search(r"https://[a-z0-9-]+\.trycloudflare\.com", line)
        if m:
            url = m.group(0)
            break
        if time.monotonic() > deadline:
            break
    if not url:
        raise RuntimeError("cloudflared did not report a tunnel address")
    # Keep draining its output so it never blocks on a full pipe.
    threading.Thread(target=lambda: [print("cloudflared:", l.rstrip(), flush=True) for l in p.stderr], daemon=True).start()
    return p, url


def main():
    global last_request
    if TOKEN.startswith("__") or TOPIC.startswith("__"):
        raise SystemExit("Push this notebook with start-cloud-ai.ps1, which sets its token and topic.")
    notify("starting")
    cloudflared = install()
    iopaint = start_iopaint()
    # The first start downloads the fill model (about 2 GB).
    deadline = time.monotonic() + 1800
    while not iopaint_ready():
        if iopaint.poll() is not None or time.monotonic() > deadline:
            notify("failed: IOPaint did not start")
            raise RuntimeError("IOPaint did not start")
        time.sleep(5)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", PROXY_PORT), Proxy)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    tunnel, url = start_tunnel(cloudflared)
    # The tunnel's address takes a few seconds to resolve.
    time.sleep(10)
    last_request = time.monotonic()
    notify(f"ready {url}")
    log("ready at", url)
    while not stop.is_set():
        if time.monotonic() - last_request > IDLE_MINUTES * 60:
            log(f"no requests for {IDLE_MINUTES} minutes, stopping")
            break
        if iopaint.poll() is not None or tunnel.poll() is not None:
            log("IOPaint or the tunnel stopped")
            break
        stop.wait(15)
    notify("stopped")
    tunnel.terminate()
    iopaint.terminate()
    server.shutdown()


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        notify(f"failed: {e}")
        raise
