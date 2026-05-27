# 03 - Provider adapters and routing

## Goal

Every provider adapter (OpenAI, Anthropic, Google, Bedrock) speaks the
OpenAI-compatible response shape on the way out. Routing primary-only,
primary + ordered fallback, and weighted split all behave as documented.
Streaming + cancellation works end-to-end.

## Setup

`marg-provider-stub` is launched once per protocol (`openai`, `anthropic`,
`google`, `bedrock`) on distinct local ports. `marg.toml` registers all
four. Routes table covers: a primary-only route, a primary + ordered
fallback route, and a weighted split route.

## Steps

```walkthrough
# 1. OpenAI stub: non-stream + stream
PROBE POST /v1/chat/completions 200 bearer $KEY body @stream=false jq '.choices[0].message.content'
PROBE POST /v1/chat/completions 200 bearer $KEY body @stream=true sse 'data:' sse '[DONE]'

# 2. Anthropic stub: same shape on output
PROBE POST /v1/chat/completions 200 bearer $KEY body @model=claude-3-5-sonnet stream=false header 'expect x-marg-provider: anthropic'
PROBE POST /v1/chat/completions 200 bearer $KEY body @model=claude-3-5-sonnet stream=true

# 3. Google stub
PROBE POST /v1/chat/completions 200 bearer $KEY body @model=gemini-1.5-pro stream=false header 'expect x-marg-provider: google'
PROBE POST /v1/chat/completions 200 bearer $KEY body @model=gemini-1.5-pro stream=true

# 4. Bedrock stub (SigV4 signed with stub creds)
PROBE POST /v1/chat/completions 200 bearer $KEY body @model=anthropic.claude-3-haiku stream=false header 'expect x-marg-provider: bedrock'
PROBE POST /v1/chat/completions 200 bearer $KEY body @model=anthropic.claude-3-haiku stream=true

# 5. Failover: primary returns 502, fallback serves
STUB inject openai 502 once
PROBE POST /v1/chat/completions 200 bearer $KEY body @stream=false header 'expect x-marg-failovers: 1' header 'expect x-marg-provider: anthropic'

# 6. Streaming cancel: client drops mid-stream, upstream byte stream aborts.
STUB inject openai delay 10s
PROBE POST /v1/chat/completions 200 bearer $KEY body @stream=true close-after 200ms
ASSERT metric marg_provider_errors_total{kind="client_disconnect"} +1
ASSERT metric marg_requests_total{status="499"} +1

# 7. Weighted split: 200 requests roughly split per weights
LOOP 200 PROBE POST /v1/chat/completions 200 bearer $KEY body @model=split-target
ASSERT split provider distribution within 10% of declared weights
```

## Expected

Every adapter returns a body matching the OpenAI shape (`choices[].message`,
`usage.prompt_tokens`, `usage.completion_tokens`). Routing headers
(`x-marg-provider`, `x-marg-model`, `x-marg-failovers`, `x-marg-attempts`)
are populated on every successful response.

## Cleanup

`STUB inject ... reset` clears all injected failures.
