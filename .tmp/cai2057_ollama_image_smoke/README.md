# CAI-2057 Ollama Image Smoke

This temporary bundle is for manual mixed-provider validation.

What it tests:
- root inference still runs through your normal parent invocation context
- the `generate_image` step switches providers through `profile`
- the image-step model comes from the Ollama profile because `generate_image.model` is intentionally omitted

Files:
- `cai2057_ollama_image_smoke.json`: agent definition
- `artifacts/`: output folder for generated images

Expected profile setup:
- Your default invocation profile should already point at OpenAI if you want the parent inference step to use your normal GPT profile.
- Create an Ollama image profile, or reuse one you already have, for example:

```bash
cargo ai profile add ollama_images \
  --server ollama \
  --model x/flux2-klein:4b
```

Check the definition:

```bash
cd /Users/jpickard/Developer/cargo-ai-infra/cargo-ai/.tmp/cai2057_ollama_image_smoke
cargo ai hatch cai2057_ollama_image_smoke --config ./cai2057_ollama_image_smoke.json --check
```

Build the temp agent:

```bash
cd /Users/jpickard/Developer/cargo-ai-infra/cargo-ai/.tmp/cai2057_ollama_image_smoke
cargo ai hatch cai2057_ollama_image_smoke --config ./cai2057_ollama_image_smoke.json
```

Run it with your default parent profile:

```bash
cd /Users/jpickard/Developer/cargo-ai-infra/cargo-ai/.tmp/cai2057_ollama_image_smoke
./cai2057_ollama_image_smoke \
  --input-override request="A vintage coffee poster with warm morning light" \
  --run-var ollama_profile=ollama_images
```

If your default profile is not the OpenAI parent profile you want, pass it explicitly:

```bash
cd /Users/jpickard/Developer/cargo-ai-infra/cargo-ai/.tmp/cai2057_ollama_image_smoke
./cai2057_ollama_image_smoke \
  --profile your_openai_parent_profile \
  --input-override request="A vintage coffee poster with warm morning light" \
  --run-var ollama_profile=ollama_images
```

Optional output override:
- The current Ollama compatibility slice requires `.png`, so keep `output_name` as a PNG file.

```bash
./cai2057_ollama_image_smoke \
  --input-override request="A hand-drawn robot mascot on graph paper" \
  --run-var ollama_profile=ollama_images \
  --run-var output_name=robot-smoke.png
```
