export function connectionSnippet(language: 'python' | 'javascript', origin: string, local: boolean, model: string): string {
  const token = local ? 'JEV_OBSERVER_ACCESS_TOKEN' : 'JEV_OBSERVER_CLIENT_TOKEN';
  const chosenModel = model.trim() || 'YOUR_MODEL_NAME';
  if (language === 'python') return `import os
from typesafe_sdk import TypeSafeClient, Noul, RetryPolicy

client = TypeSafeClient(
    api_key=os.environ[${JSON.stringify(token)}],
    base_url=${JSON.stringify(origin)},
    model=${JSON.stringify(chosenModel)},
    retry=RetryPolicy(max_retries=0),
    headers={
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
  defaultHeaders: {
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
