//! `trace start` / `trace stop` and `profile start` / `profile stop` on the current page.

use super::commands::evaluate;
use super::State;
use crate::cdp::Cdp;
use crate::fields;
use crate::output::{Failure, Fields, Kind, Outcome};
use base64::Engine as _;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A trace or profile between its `start` and `stop`.
#[derive(Clone)]
pub(super) struct Recording {
    target: String,
    /// `performance.timeOrigin` of the page when recording started.
    origin: f64,
}

/// How long `trace stop` waits for the browser to finish the trace and hand over its stream.
const TRACE_COMPLETE_WAIT: Duration = Duration::from_secs(30);

impl State {
    pub(super) async fn trace_start_cmd(&self) -> Outcome {
        if self.tracing.lock().unwrap().is_some() {
            return Err(Failure::new(
                Kind::Error,
                "a trace is already running; run `trace stop` first",
            ));
        }
        let (target, cdp) = self.current()?;
        self.ensure_ready(&target, &cdp).await?;
        let origin = time_origin(&cdp).await?;
        cdp.call(
            "Tracing.start",
            json!({
                "transferMode": "ReturnAsStream",
                "streamFormat": "json",
                "traceConfig": {
                    "includedCategories": crate::trace::CATEGORIES,
                    "excludedCategories": ["*"],
                },
            }),
        )
        .await?;
        *self.tracing.lock().unwrap() = Some(Recording {
            target: target.clone(),
            origin,
        });
        Ok(fields! { "target" => target, "categories" => crate::trace::CATEGORIES })
    }

    pub(super) async fn trace_stop_cmd(&self) -> Outcome {
        let Some(rec) = self.tracing.lock().unwrap().clone() else {
            return Err(Failure::new(
                Kind::Error,
                "no trace is running; run `trace start` first",
            ));
        };
        let cdp = self.cdp(&rec.target)?;
        let complete = self.expect_event(&rec.target, "Tracing.tracingComplete");
        cdp.call("Tracing.end", json!({})).await?;
        *self.tracing.lock().unwrap() = None;
        let done = tokio::time::timeout(TRACE_COMPLETE_WAIT, complete)
            .await
            .map_err(|_| {
                Failure::new(
                    Kind::Timeout,
                    format!(
                        "the trace did not complete within {}s",
                        TRACE_COMPLETE_WAIT.as_secs()
                    ),
                )
            })?
            .map_err(|_| Failure::new(Kind::Error, "the page closed before the trace completed"))?;
        let Some(handle) = done["stream"].as_str() else {
            return Err(Failure::new(
                Kind::Error,
                "the trace came back without a stream",
            ));
        };
        let bytes = read_stream(&cdp, handle).await?;
        let path = self.record_file("trace", "json");
        write(&path, &bytes)?;
        check_same_document(&cdp, &rec, &path).await?;
        let trace: Value = serde_json::from_slice(&bytes)
            .map_err(|e| Failure::new(Kind::Error, format!("the trace is not JSON: {e}")))?;
        let events = crate::trace::events(&trace);
        let mut f = crate::trace::summarize(events);
        f.insert("events".into(), json!(events.len()));
        f.insert("data_loss".into(), done["dataLossOccurred"].clone());
        Ok(with_file(f, &path, &rec))
    }

    pub(super) async fn profile_start_cmd(&self, interval_us: u64) -> Outcome {
        if interval_us == 0 {
            return Err(Failure::new(
                Kind::Error,
                "profile needs --interval-us of at least 1",
            ));
        }
        if self.profiling.lock().unwrap().is_some() {
            return Err(Failure::new(
                Kind::Error,
                "a profile is already running; run `profile stop` first",
            ));
        }
        let (target, cdp) = self.current()?;
        self.ensure_ready(&target, &cdp).await?;
        let origin = time_origin(&cdp).await?;
        cdp.call("Profiler.enable", json!({})).await?;
        cdp.call(
            "Profiler.setSamplingInterval",
            json!({ "interval": interval_us }),
        )
        .await?;
        cdp.call("Profiler.start", json!({})).await?;
        *self.profiling.lock().unwrap() = Some(Recording {
            target: target.clone(),
            origin,
        });
        Ok(fields! { "target" => target, "interval_us" => interval_us })
    }

    pub(super) async fn profile_stop_cmd(&self, top: usize) -> Outcome {
        let Some(rec) = self.profiling.lock().unwrap().clone() else {
            return Err(Failure::new(
                Kind::Error,
                "no profile is running; run `profile start` first",
            ));
        };
        let cdp = self.cdp(&rec.target)?;
        let mut stopped = cdp.call("Profiler.stop", json!({})).await?;
        *self.profiling.lock().unwrap() = None;
        let _ = cdp.call("Profiler.disable", json!({})).await;
        let profile = stopped["profile"].take();
        if !profile.is_object() {
            return Err(Failure::new(
                Kind::Error,
                "the profiler returned no profile",
            ));
        }
        let path = self.record_file("profile", "cpuprofile");
        write(&path, profile.to_string().as_bytes())?;
        check_same_document(&cdp, &rec, &path).await?;
        Ok(with_file(
            crate::profile::summarize(&profile, top),
            &path,
            &rec,
        ))
    }

    /// A new file in the run record, numbered like the record's next entry.
    fn record_file(&self, stem: &str, ext: &str) -> PathBuf {
        let seq = *self.record_seq.lock().unwrap() + 1;
        self.record_dir.join(format!("{stem}-{seq}.{ext}"))
    }
}

async fn time_origin(cdp: &Cdp) -> Result<f64, Failure> {
    let v = evaluate(cdp, "performance.timeOrigin").await?;
    v.as_f64()
        .ok_or_else(|| Failure::new(Kind::Error, "the page did not report its time origin"))
}

/// Fails the run when the page's document is not the one recording started in; the recording
/// is still saved at `path`.
async fn check_same_document(cdp: &Cdp, rec: &Recording, path: &Path) -> Result<(), Failure> {
    let now = time_origin(cdp).await.ok();
    if now == Some(rec.origin) {
        return Ok(());
    }
    Err(Failure::new(
        Kind::Reloaded,
        "the page reloaded during the recording; this run is void",
    )
    .with("time_origin_at_start", rec.origin)
    .with("time_origin_now", now)
    .with("path", path.display().to_string()))
}

fn with_file(mut f: Fields, path: &Path, rec: &Recording) -> Fields {
    f.insert("path".into(), json!(path.display().to_string()));
    f.insert("target".into(), json!(rec.target));
    f
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    std::fs::write(path, bytes).map_err(|e| {
        Failure::new(
            Kind::Error,
            format!("could not write {}: {e}", path.display()),
        )
    })
}

/// Reads a protocol stream to its end; the stream is closed whether or not reading succeeds.
async fn read_stream(cdp: &Cdp, handle: &str) -> Result<Vec<u8>, Failure> {
    let read = async {
        let mut bytes = Vec::new();
        loop {
            let chunk = cdp
                .call("IO.read", json!({ "handle": handle, "size": 1 << 20 }))
                .await?;
            let data = chunk["data"].as_str().unwrap_or("");
            if chunk["base64Encoded"] == true {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| {
                        Failure::new(Kind::Error, format!("the trace stream is not base64: {e}"))
                    })?;
                bytes.extend(decoded);
            } else {
                bytes.extend(data.as_bytes());
            }
            if chunk["eof"] == true {
                return Ok::<_, Failure>(bytes);
            }
        }
    };
    let bytes = read.await;
    let _ = cdp.call("IO.close", json!({ "handle": handle })).await;
    bytes
}
