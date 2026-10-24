# Cloud AI on Kaggle

Prompted Generative Fill needs a diffusion model that does not fit beside PhotoCraft on a 4 GB card.
This runs it on Kaggle's free GPU instead (a T4 with 16 GB, about 30 hours a week). Object
removal and Remove Background stay on the local AI server (`packaging/ai-server`) and only use the
cloud one while the local one is not running.

How it fits together:

- `photocraft_ai_kaggle.py` is a private Kaggle notebook. It installs IOPaint, loads the fill model,
  and puts a proxy in front that answers only requests carrying this PC's token. A Cloudflare quick
  tunnel gives it a public https address, which it posts to a private ntfy.sh topic. It stops by
  itself after 30 minutes without a fill.
- `start-cloud-ai.ps1` (the **Start Cloud AI** shortcut) pushes the notebook with the token, waits
  for the address and writes it, with the token, to `D:\AI\cloud\server.txt`.
- PhotoCraft reads that file for each prompted fill (`PHOTOCRAFT_AI_CLOUD_FILE`), so it can stay
  open while the notebook starts, stops and starts again at a new address.
- `stop-cloud-ai.ps1` (**Stop Cloud AI**) stops the notebook to save GPU hours.

## Setup

1. On kaggle.com: Settings › Phone verification (needed for internet in notebooks), then
   Settings › API › Create New Token. Save the downloaded `kaggle.json` to
   `%USERPROFILE%\.kaggle\kaggle.json` on the PC.
2. `powershell -ExecutionPolicy Bypass -File install-cloud-ai.ps1` (default folder `D:\AI`).
3. Restart PhotoCraft once.

Then: Start Cloud AI, wait for "Ready" (about 10 minutes, since each Kaggle session installs
from scratch), and use Edit › Generative Fill with a prompt.

Kaggle's terms are for machine-learning work and forbid using notebooks as general-purpose
servers; an account that breaks them can be suspended. This is a personal, low-volume use
that stops when idle, but the risk is the account owner's.
