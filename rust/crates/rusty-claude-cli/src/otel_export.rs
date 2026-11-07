//! The process's OpenTelemetry exporter ([`telemetry::otel`]), when
//! `OTEL_EXPORTER_OTLP_ENDPOINT` or the `telemetry.otel` setting turns it on.
//!
//! Every runtime the CLI builds gets a [`SessionTracer`] on the one exporter
//! ([`tracer_for`]); `https://` collectors go through the CLI's TLS client. A one-shot
//! run calls [`shutdown`] before it exits so its spans are sent.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use telemetry::otel::{OtelConfig, OtlpExporter, OtlpTransport, PlainHttpTransport};
use telemetry::SessionTracer;

static EXPORTER: OnceLock<Option<Arc<OtlpExporter>>> = OnceLock::new();

/// Plain HTTP through the built-in transport, HTTPS through `reqwest`.
struct CliTransport {
    plain: PlainHttpTransport,
    tls: Option<reqwest::blocking::Client>,
}

impl OtlpTransport for CliTransport {
    fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<u16, String> {
        if url.starts_with("http://") {
            return self.plain.post_json(url, headers, body);
        }
        let client = self
            .tls
            .as_ref()
            .ok_or_else(|| "no TLS client for the OTLP endpoint".to_string())?;
        let mut request = client
            .post(url)
            .header("content-type", "application/json")
            .body(body.to_vec());
        for (key, value) in headers {
            request = request.header(key.as_str(), value.as_str());
        }
        // The error text can contain the URL but never the headers.
        request
            .send()
            .map(|response| response.status().as_u16())
            .map_err(|_| "OTLP export request failed".to_string())
    }
}

/// The `telemetry.otel` setting, as JSON.
fn otel_setting() -> Option<serde_json::Value> {
    let cwd = std::env::current_dir().ok()?;
    let config = runtime::ConfigLoader::default_for(&cwd).load().ok()?;
    let rendered = config.get("telemetry")?.render();
    let telemetry: serde_json::Value = serde_json::from_str(&rendered).ok()?;
    telemetry.get("otel").cloned()
}

fn exporter() -> Option<&'static Arc<OtlpExporter>> {
    EXPORTER
        .get_or_init(|| {
            let config = match OtelConfig::from_env(otel_setting().as_ref()) {
                Ok(Some(config)) => config,
                Ok(None) => return None,
                Err(reason) => {
                    runtime::ui_hooks::notice(&format!("OpenTelemetry export is off: {reason}"));
                    return None;
                }
            };
            let tls = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .ok();
            let transport = CliTransport {
                plain: PlainHttpTransport::default(),
                tls,
            };
            Some(Arc::new(OtlpExporter::with_transport(
                config,
                Box::new(transport),
            )))
        })
        .as_ref()
}

/// A tracer for `session_id` on the process's exporter; `None` when export is off.
pub(crate) fn tracer_for(session_id: &str) -> Option<SessionTracer> {
    let exporter = exporter()?;
    Some(SessionTracer::new(
        session_id,
        Arc::clone(exporter) as Arc<dyn telemetry::TelemetrySink>,
    ))
}

/// Sends what is left (waiting at most a few seconds). Only when export was set up.
pub(crate) fn shutdown() {
    if let Some(Some(exporter)) = EXPORTER.get() {
        exporter.shutdown(Duration::from_secs(3));
    }
}
