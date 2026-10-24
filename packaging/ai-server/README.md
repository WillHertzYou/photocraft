# PhotoCraft AI server

PhotoCraft's AI features talk to a local [IOPaint](https://github.com/Sanster/IOPaint) server:

| Feature | Where | Model |
|---|---|---|
| Generative Fill with a prompt | Edit › Generative Fill… | Realistic Vision 5.1 inpainting (Stable Diffusion 1.5) |
| Generative Fill with an empty prompt (remove an object) | Edit › Generative Fill… | LaMa |
| Remove Background | Properties › Quick Actions | BRIA RMBG-1.4 |

Everything runs on the PC; nothing is uploaded. Generative Fill puts the result on a new layer
masked to the selection, as Photoshop does, so it can be undone, hidden or repainted. Remove
Background falls back to PhotoCraft's own (non-AI) method when the server isn't running.

## Set up (Windows, NVIDIA)

```powershell
powershell -ExecutionPolicy Bypass -File packaging\ai-server\install-ai-server.ps1 -AiDir D:\AI
powershell -ExecutionPolicy Bypass -File D:\AI\start-ai-server.ps1
```

Then start PhotoCraft. Settings, as environment variables:

- `PHOTOCRAFT_AI_URL`: the server, default `http://127.0.0.1:8080`.
- `PHOTOCRAFT_AI_FILL_MODEL`: the model for prompted fills.

## What to expect on older hardware

Measured on a GTX 980 (4 GB) with an i7-930 (no AVX) and 8 GB of RAM, 512×512 areas:

| Task | Time |
|---|---|
| Remove an object (empty prompt, LaMa) | about 20 s |
| Remove Background (RMBG-1.4, on the CPU) | a few seconds |
| Generative Fill with a prompt (25 steps) | about 100 s, plus about 35 s the first time while the model loads |

The server keeps one model loaded, so switching between object removal and prompted fills
reloads a model. Prompted fills are Stable Diffusion 1.5 quality: good for skies, grass,
textures and simple objects, weaker than Adobe Firefly for people, hands and text.

### Known limit: prompted fills on a 4 GB card with 8 GB of RAM

On that PC, object removal and Remove Background work inside PhotoCraft (tested end to end), but
a prompted fill fills the card's video memory (3.9 of 4 GB measured, with the safety filter on or
off) and, with other programs open, the system RAM as well. PhotoCraft's window then lost its GPU
device during the fill; it saved the document for recovery and closed, as it is designed to, and
the next launch reopened it. After each prompted fill PhotoCraft now switches the server back to
the small LaMa model (`PHOTOCRAFT_AI_KEEP_FILL_MODEL=1` keeps the big one loaded). For prompted
fills on such a PC: close other programs first, add RAM, or use a cloud service.

Notes for Maxwell cards: use the CUDA 12.6 PyTorch build (`install-ai-server.ps1` does); later
builds dropped these GPUs. If `CUDA_VISIBLE_DEVICES` is set to `-1` in the Windows environment,
CUDA programs can't see the card; the start script overrides it for the server only.
