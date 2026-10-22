import type { Protocol } from './types';

export const protocolLabel = (protocol: Protocol) => ({ systemone: 'System One', 'openai-decisions': 'OpenAI Decisions', 'cloudflare-systemone': 'Cloudflare System One' })[protocol];
export const protocolEndpoint = (protocol: Protocol) => protocol === 'openai-decisions' ? '/v1/decisions' : '/v1/systemone';

export function connectionSnippet(language: 'python' | 'javascript', origin: string, local: boolean, model: string, longTimeout = local, protocol: Protocol = 'systemone'): string {
  const token = local ? 'JEV_OBSERVER_ACCESS_TOKEN' : 'JEV_OBSERVER_CLIENT_TOKEN';
  const chosenModel = model.trim() || 'YOUR_MODEL_NAME';
  if (protocol === 'openai-decisions') return openAIDecisionsSnippet(language, origin, token, chosenModel, longTimeout);
  if (protocol === 'cloudflare-systemone') return cloudflareSnippet(language, origin, token, chosenModel, longTimeout);
  if (language === 'python') return `import os
from typesafe_sdk import TypeSafeClient, Noul, RetryPolicy

client = TypeSafeClient(
    api_key=os.environ[${JSON.stringify(token)}],
    base_url=${JSON.stringify(origin)},
    model=${JSON.stringify(chosenModel)},
    retry=RetryPolicy(max_retries=0),
${longTimeout ? '    timeout=180,\n' : ''}    headers={
        "x-observer-source": "my-application",
        "Accept-Encoding": "identity",
    },
)
response = client.system_one(
    state={"message": "Please help me reset my password."},
    questions={"needs_help": Noul(instructions="Does the message ask for help?")},
)
print(response)`;
  return `import { TypeSafeClient } from "@typesafe-ai/sdk";

const client = new TypeSafeClient({
  apiKey: process.env.${token},
  baseURL: ${JSON.stringify(origin)},
  retry: { maxRetries: 0 },
${longTimeout ? '  timeout: 180000,\n' : ''}  defaultHeaders: {
    "x-observer-source": "my-application",
    "Accept-Encoding": "identity",
  },
});
const response = await client.systemOne({
  model: ${JSON.stringify(chosenModel)},
  state: { message: "Please help me reset my password." },
  questions: {
    needs_help: { type: "noul", instructions: "Does the message ask for help?" },
  },
});
console.log(response);`;
}

function openAIDecisionsSnippet(language: 'python' | 'javascript', origin: string, token: string, model: string, longTimeout: boolean): string {
  if (language === 'python') return `import os
from openai import OpenAI

client = OpenAI(
    api_key=os.environ[${JSON.stringify(token)}],
    base_url=${JSON.stringify(origin + '/v1')},
    max_retries=0,
    timeout=${longTimeout ? 180 : 60},
    default_headers={"x-observer-source": "my-application", "Accept-Encoding": "identity"},
)
response = client.decisions.create(
    model=${JSON.stringify(model)},
    input="Please help me reset my password.",
    questions=[{"type": "predicate", "name": "needs_help", "instructions": "Does the message ask for help?"}],
)
print(response)`;
  return `import OpenAI from "openai";

const client = new OpenAI({
  apiKey: process.env.${token},
  baseURL: ${JSON.stringify(origin + '/v1')},
  maxRetries: 0,
  timeout: ${longTimeout ? 180000 : 60000},
  defaultHeaders: { "x-observer-source": "my-application", "Accept-Encoding": "identity" },
});
const response = await client.decisions.create({
  model: ${JSON.stringify(model)},
  input: "Please help me reset my password.",
  questions: [{ type: "predicate", name: "needs_help", instructions: "Does the message ask for help?" }],
});
console.log(response);`;
}

function cloudflareSnippet(language: 'python' | 'javascript', origin: string, token: string, model: string, longTimeout: boolean): string {
  if (language === 'python') return `import json
import os
from urllib.request import Request, urlopen

payload = {
    "model": ${JSON.stringify(model)},
    "state": {"message": "Please help me reset my password."},
    "questions": {"needs_help": {"type": "noul", "instructions": "Does the message ask for help?"}},
}
request = Request(
    ${JSON.stringify(origin + '/v1/systemone')},
    data=json.dumps(payload).encode("utf-8"),
    headers={
        "Authorization": "Bearer " + os.environ[${JSON.stringify(token)}],
        "Content-Type": "application/json",
        "Accept-Encoding": "identity",
        "x-observer-source": "my-application",
    },
)
with urlopen(request, timeout=${longTimeout ? 180 : 60}) as response:
    envelope = json.load(response)
if envelope.get("success") is not True:
    raise RuntimeError("Cloudflare reported an unsuccessful response")
print(envelope["result"])`;
  return `const response = await fetch(${JSON.stringify(origin + '/v1/systemone')}, {
  method: "POST",
  signal: AbortSignal.timeout(${longTimeout ? 180000 : 60000}),
  headers: {
    Authorization: "Bearer " + process.env.${token},
    "Content-Type": "application/json",
    "Accept-Encoding": "identity",
    "x-observer-source": "my-application",
  },
  body: JSON.stringify({
    model: ${JSON.stringify(model)},
    state: { message: "Please help me reset my password." },
    questions: { needs_help: { type: "noul", instructions: "Does the message ask for help?" } },
  }),
});
if (!response.ok) throw new Error("Observer returned HTTP " + response.status);
const envelope = await response.json();
if (envelope.success !== true) throw new Error("Cloudflare reported an unsuccessful response");
console.log(envelope.result);`;
}
