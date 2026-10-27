# Captured provider requests (redacted)

| field | sdk-fresh-session | sdk-resumed-session | sdk-legacy-snapshot | kernel-round |
| --- | --- | --- | --- | --- |
| method | "POST" | "POST" | "POST" | "POST" |
| path | "/v1/chat/completions" | "/v1/chat/completions" | "/v1/chat/completions" | "/v1/chat/completions" |
| query | "" | "" | "" | "" |
| model | "sensenova-6.8-flash-lite" | "sensenova-6.8-flash-lite" | "sensenova-6.8-flash-lite" | "sensenova-6.8-flash-lite" |
| stream | true | true | true | true |
| maxTokens | 65535 | 65535 | 65535 | 65535 |
| streamOptions | {"include_usage":true} | {"include_usage":true} | {"include_usage":true} | {"include_usage":true} |
| store | null | null | null | null |
| authorizationScheme | "Bearer" | "Bearer" | "Bearer" | "Bearer" |

No routing-relevant difference: method, path, query, model, stream flag, token budget field/value, stream options, store flag and authorization scheme are identical.

## Outcomes against the stub

- sdk-fresh-session: run.completed
- sdk-resumed-session: run.completed
- sdk-legacy-snapshot: run.completed
- kernel-round: kernel.model_response
