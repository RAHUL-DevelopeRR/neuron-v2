# OpenCode & External Application Integration Guide

Connect **OpenCode**, **Cursor**, **Zed**, **Continue.dev**, or any custom application directly to the **Zero-X Multi-Provider AI Gateway**.

---

## 1. Gateway Connection Parameters

* **Production Endpoint**: `https://zero-x.live/v1`
* **Local Development Endpoint**: `http://127.0.0.1:8787/v1`
* **Authorization**: `Bearer <session_token>`
* **Supported Models**:
  * `gemini/gemini-2.5-flash` (Google Gemini)
  * `gemini/gemini-2.5-pro` (Google Gemini)
  * `groq/openai/gpt-oss-120b` (Groq Cloud)
  * `groq/openai/gpt-oss-20b` (Groq Cloud)
  * `groq/qwen/qwen3.8-27b` (Groq Cloud)
  * `nvidia/meta/llama-3.2-11b-vision-instruct` (NVIDIA NIM)
  * `nvidia/deepseek-ai/deepseek-coder-6.7b-instruct` (NVIDIA NIM)
  * `cohere/north-mini-code:free` (OpenRouter)
  * `@cf/meta/llama-3.3-70b-instruct-fp8-fast` (Cloudflare Workers AI)

---

## 2. Connecting OpenCode (`sst/opencode`)

OpenCode can connect to any OpenAI-compatible custom provider endpoint via environment variables or configuration file.

### Option A: Shell Environment Variables
```bash
export OPENAI_BASE_URL="https://zero-x.live/v1"
export OPENAI_API_KEY="ses_your_session_token_here"

# Launch OpenCode
opencode
```

### Option B: OpenCode Configuration File (`opencode.json`)
Create an `opencode.json` in your project root or `~/.config/opencode/config.json`:
```json
{
  "provider": "openai-compatible",
  "endpoint": "https://zero-x.live/v1",
  "apiKey": "ses_your_session_token_here",
  "model": "gemini/gemini-2.5-flash",
  "fallbackModel": "groq/openai/gpt-oss-120b"
}
```

---

## 3. Connecting Cursor / Zed / Continue.dev

### In Cursor / Zed:
1. Open **Settings** -> **AI** -> **OpenAI API Key / Custom URL**.
2. Set **Base URL**: `https://zero-x.live/v1`
3. Set **API Key**: `ses_your_session_token_here`
4. Set Model: `gemini/gemini-2.5-flash` or `groq/openai/gpt-oss-120b`.

### In Continue.dev (`config.json`):
```json
{
  "models": [
    {
      "title": "Zero-X Gemini 2.5 Flash",
      "provider": "openai",
      "model": "gemini/gemini-2.5-flash",
      "apiBase": "https://zero-x.live/v1",
      "apiKey": "ses_your_session_token_here"
    },
    {
      "title": "Zero-X Groq Llama 3.3",
      "provider": "openai",
      "model": "groq/openai/gpt-oss-120b",
      "apiBase": "https://zero-x.live/v1",
      "apiKey": "ses_your_session_token_here"
    }
  ]
}
```

---

## 4. Connecting via Python SDK

```python
import os
from openai import OpenAI

client = OpenAI(
    base_url="https://zero-x.live/v1",
    api_key=os.environ.get("NEURON_TOKEN", "ses_your_session_token_here")
)

response = client.chat.completions.create(
    model="gemini/gemini-2.5-flash",
    messages=[
        {"role": "system", "content": "You are an expert software engineer."},
        {"role": "user", "content": "Write a fast Python script to benchmark API latency."}
    ],
    stream=True
)

for chunk in response:
    content = chunk.choices[0].delta.content or ""
    print(content, end="", flush=True)
```

---

## 5. Connecting via Node.js SDK

```javascript
import OpenAI from 'openai';

const openai = new OpenAI({
  baseURL: 'https://zero-x.live/v1',
  apiKey: process.env.NEURON_TOKEN || 'ses_your_session_token_here',
});

const stream = await openai.chat.completions.create({
  model: 'gemini/gemini-2.5-flash',
  messages: [{ role: 'user', content: 'Explain Zero-X architecture in 3 bullet points.' }],
  stream: true,
});

for await (const chunk of stream) {
  process.stdout.write(chunk.choices[0]?.delta?.content || '');
}
```
