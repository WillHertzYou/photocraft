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

Measured on a GTX 980 (4 GB) with an i7-930 and 8 GB of RAM. The server keeps one model loaded;
the first prompted fill after an object removal loads the diffusion model, which takes longest.
Prompted fills are Stable Diffusion 1.5 quality: good for skies, grass, textures and simple
objects, weaker than Adobe Firefly for people, hands and text.
