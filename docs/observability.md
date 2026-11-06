# OpenTelemetry export and artifacts

## OpenTelemetry

Neuron can export spans and metrics over OTLP/HTTP with JSON bodies. It is off unless
one of these is set:

- `OTEL_EXPORTER_OTLP_ENDPOINT` (e.g. `http://localhost:4318`); the per-signal
  `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` / `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` are used as
  given;
- the `telemetry.otel` setting: `true` (sends to `http://localhost:4318`), an endpoint
  URL, or `{"enabled": true, "endpoint": "...", "serviceName": "...", "logContent": false}`.

`"telemetry": {"otel": false}` or `OTEL_SDK_DISABLED=true` turns it off whatever the
environment says. `OTEL_EXPORTER_OTLP_HEADERS` (`key=value,...`, for auth) and
`OTEL_SERVICE_NAME` are honoured; `OTEL_EXPORTER_OTLP_PROTOCOL=grpc` is refused (only
OTLP/HTTP). Both `http://` and `https://` collectors work.

Spans: `neuron.turn` (root), `neuron.model_request` (client), `neuron.tool_call`, with
`session.id`, `tool.name` and numeric attributes. Metrics (delta): `neuron.tokens`
(by `token.type` = input/output/cache_read/cache_creation and `model`),
`neuron.model_request.duration`, `neuron.tool_call.duration`, `neuron.turn.duration`
(ms histograms).

Prompt, response and tool text are **never** exported by default. `logContent: true` in
the setting (or `NEURON_OTEL_LOG_CONTENT=1`) adds the prompt and error messages.

Export runs on a background thread every 2 s; failures are counted, never printed.

## Artifacts

Tool outputs longer than the context cap (40,000 characters) are kept whole in a
content-addressed store, `<NEURON_HOME>/artifacts/<session>/` (`manifest.json` plus
`objects/ab/cdef...`). The model gets the usual head-and-tail preview followed by the
artifact id and path. `/artifacts` lists the session's artifacts.

Caps: 32 MiB per artifact, 256 MiB per session; beyond them the output is only
truncated, as before. Session folders idle for 14 days are removed. `NEURON_ARTIFACTS=0`
turns the store off.
