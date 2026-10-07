//! Performance: the page clock, `hold`, and `measure start` / `measure stop`.

use super::commands::{evaluate, evaluate_within};
use super::State;
use crate::cdp::{Cdp, Reply};
use crate::chord;
use crate::clock::{self, Clock, Sample};
use crate::fields;
use crate::measure::{self, Thresholds, Watch};
use crate::output::{Failure, Fields, Kind, Outcome};
use crate::request::{Route, HOLD_ACK_WAIT};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// What `hold` was asked to do.
#[derive(Clone, Copy)]
pub(super) struct Hold<'a> {
    pub(super) chord: &'a str,
    pub(super) count: u32,
    pub(super) interval_ms: u64,
    pub(super) into: Option<&'a str>,
    pub(super) require_focus: Option<&'a str>,
    pub(super) route: Route,
}

/// A measurement between `measure start` and `measure stop`.
#[derive(Clone)]
pub(super) struct Measuring {
    /// Tags this measurement's probes in the page, so removing them never touches another's.
    id: String,
    target: String,
    origin: f64,
    long_frame_ms: f64,
    over_ms: f64,
    /// Page-clock times (epoch ms) of the key-downs `hold` sent during the measurement.
    key_downs: Vec<f64>,
}

/// How long removing probes may take when the session is ending.
const REMOVE_WAIT: Duration = Duration::from_secs(2);

impl State {
    /// Measures the page clock of `target` against pokit's and keeps it for stamping input.
    pub(super) async fn align_clock(&self, target: &str, cdp: &Cdp) -> Result<Clock, Failure> {
        let mut samples = Vec::with_capacity(clock::SAMPLES);
        for _ in 0..clock::SAMPLES {
            let sent = Instant::now();
            let page = evaluate(cdp, "performance.timeOrigin + performance.now()").await?;
            let received = Instant::now();
            if let Some(page) = page.as_f64() {
                samples.push(Sample {
                    sent: clock::local_ms(sent),
                    page,
                    received: clock::local_ms(received),
                });
            }
        }
        let c = clock::align(&samples)
            .ok_or_else(|| Failure::new(Kind::Error, "the page did not report its clock"))?;
        *self.clock.lock().unwrap() = Some((target.to_string(), c));
        Ok(c)
    }

    /// The latest clock alignment and the target it was taken on.
    pub(super) fn clock_json(&self) -> Value {
        match &*self.clock.lock().unwrap() {
            Some((target, c)) => {
                let mut v = c.to_json();
                v["target"] = json!(target);
                v
            }
            None => json!("no clock alignment for this session"),
        }
    }

    /// Adds when each input event was sent, on the page clock, and the alignment used.
    pub(super) fn stamp(&self, mut f: Fields, sent: &[Instant]) -> Fields {
        let clock = self.clock.lock().unwrap().as_ref().map(|(_, c)| *c);
        let at = match clock {
            Some(c) => json!(sent
                .iter()
                .map(|i| clock::round3(c.page_time(clock::local_ms(*i))))
                .collect::<Vec<f64>>()),
            None => json!("no clock alignment for this session"),
        };
        f.insert("sent_at".into(), at);
        f.insert("clock".into(), self.clock_json());
        f
    }

    /// Sends `count` key-downs `interval_ms` apart without waiting for the page, then one key-up,
    /// on CDP or through the OS. Key-downs sent during a measurement are kept for its latency.
    pub(super) async fn hold_cmd(&self, hold: Hold<'_>) -> Outcome {
        let press = chord::parse_chord(hold.chord).map_err(|e| Failure::new(Kind::Error, e))?;
        if hold.count == 0 {
            return Err(Failure::new(
                Kind::Error,
                "hold needs --count of at least 1",
            ));
        }
        let interval = Duration::from_millis(hold.interval_ms);
        let (sent, acknowledged) = match hold.route {
            Route::Cdp => {
                let cdp = self.focus_for_input(hold.into, hold.require_focus).await?;
                self.cdp_hold(&cdp, &press, hold.count, interval).await?
            }
            #[cfg(windows)]
            Route::Os => {
                let pid = self.refuse_unless_front().await?;
                self.focus_for_input(hold.into, hold.require_focus).await?;
                let sent = self.os_hold(pid, &press, hold.count, interval).await?;
                let downs = sent.len().saturating_sub(1);
                (sent, downs)
            }
            #[cfg(not(windows))]
            Route::Os => {
                return Err(Failure::new(
                    Kind::Unsupported,
                    "OS input is not built on this platform yet",
                ))
            }
        };
        let (target, _) = self.current()?;
        self.note_key_downs(&target, &sent[..hold.count as usize]);
        let f = fields! {
            "key" => press.key,
            "code" => press.code,
            "modifiers" => press.modifiers,
            "count" => hold.count,
            "interval_ms" => hold.interval_ms,
            "acknowledged" => acknowledged,
            "route" => match hold.route { Route::Cdp => "cdp", Route::Os => "os" },
        };
        Ok(self.stamp(f, &sent))
    }

    /// The CDP side of `hold`: the modifiers down, every key-down and the key-up on schedule, the
    /// modifiers up, the replies awaited only after the last; when each key-down and the key-up
    /// were sent, and how many of the chord's key events the page acknowledged.
    async fn cdp_hold(
        &self,
        cdp: &Cdp,
        press: &chord::KeyPress,
        count: u32,
        interval: Duration,
    ) -> Result<(Vec<Instant>, usize), Failure> {
        let (modifier_downs, modifier_ups) = press.modifier_events();
        let mut sent = Vec::with_capacity(count as usize + 1);
        let mut replies: Vec<Reply> = Vec::new();
        let pressed = async {
            for event in modifier_downs {
                replies.push(cdp.send("Input.dispatchKeyEvent", event)?);
            }
            let start = tokio::time::Instant::now();
            for n in 0..=count {
                tokio::time::sleep_until(start + interval * n).await;
                let event = if n < count {
                    press.down_event(n > 0)
                } else {
                    press.up_event()
                };
                sent.push(Instant::now());
                replies.push(cdp.send("Input.dispatchKeyEvent", event)?);
            }
            Ok::<_, Failure>(())
        }
        .await;
        if let Err(f) = pressed {
            let _ = cdp.send("Input.dispatchKeyEvent", press.up_event());
            for event in modifier_ups {
                let _ = cdp.send("Input.dispatchKeyEvent", event);
            }
            return Err(f);
        }
        for event in modifier_ups {
            replies.push(cdp.send("Input.dispatchKeyEvent", event)?);
        }
        let deadline = Instant::now() + HOLD_ACK_WAIT;
        let mut acknowledged = 0;
        for reply in replies {
            reply
                .wait(deadline.saturating_duration_since(Instant::now()))
                .await?;
            acknowledged += 1;
        }
        Ok((sent, acknowledged))
    }

    /// Keeps the page-clock times of key-downs sent to `target` while a measurement runs there,
    /// for its latency.
    fn note_key_downs(&self, target: &str, sent: &[Instant]) {
        let Some(c) = self.clock.lock().unwrap().as_ref().map(|(_, c)| *c) else {
            return;
        };
        if let Some(m) = self.measuring.lock().unwrap().as_mut() {
            if m.target != target {
                return;
            }
            m.key_downs
                .extend(sent.iter().map(|i| c.page_time(clock::local_ms(*i))));
        }
    }

    /// `hold --compare`: the same keys on CDP and then through the OS, each inside its own
    /// measurement, and how much longer the OS route took from send to handling.
    pub(super) async fn hold_compare_cmd(&self, hold: Hold<'_>) -> Outcome {
        if self.measuring.lock().unwrap().is_some() {
            return Err(Failure::new(
                Kind::GuardRefused,
                "a measurement is running, and --compare takes its own; run `measure stop` first",
            ));
        }
        if cfg!(not(windows)) {
            return Err(Failure::new(
                Kind::Unsupported,
                "OS input is not built on this platform yet",
            ));
        }
        #[cfg(windows)]
        self.refuse_unless_front().await?;
        let mut runs = serde_json::Map::new();
        for route in [Route::Cdp, Route::Os] {
            self.measure_start_cmd(
                None,
                None,
                Thresholds::default().long_frame_ms,
                Thresholds::default().over_ms,
            )
            .await?;
            let held = self.hold_cmd(Hold { route, ..hold }).await;
            let stopped = self
                .measure_stop_cmd(300, crate::request::COMPARE_SETTLE.as_millis() as u64)
                .await;
            held?;
            let m = stopped?;
            let name = match route {
                Route::Cdp => "cdp",
                Route::Os => "os",
            };
            runs.insert(
                name.into(),
                json!({ "latency": m["latency"], "keys": m["keys"], "frames": m["frames"] }),
            );
        }
        let diff = |p: &str| match (
            runs["os"]["latency"][p].as_f64(),
            runs["cdp"]["latency"][p].as_f64(),
        ) {
            (Some(os), Some(cdp)) => json!((((os - cdp) * 10.0).round() / 10.0) + 0.0),
            _ => json!("a route paired no keys"),
        };
        let difference = json!({ "p50_ms": diff("p50_ms"), "p95_ms": diff("p95_ms") });
        let mut f = Fields::new();
        f.insert("cdp".into(), runs["cdp"].clone());
        f.insert("os".into(), runs["os"].clone());
        f.insert("os_minus_cdp".into(), difference);
        f.insert("clock".into(), self.clock_json());
        Ok(f)
    }

    pub(super) async fn measure_start_cmd(
        &self,
        watch: Option<&str>,
        watch_attr: Option<&str>,
        long_frame_ms: f64,
        over_ms: f64,
    ) -> Outcome {
        let watch = match (watch, watch_attr) {
            (Some(s), attr) => Some(Watch {
                selector: s.to_string(),
                attr: attr.map(str::to_string),
            }),
            (None, Some(_)) => {
                return Err(Failure::new(Kind::Error, "--watch-attr needs --watch"));
            }
            (None, None) => None,
        };
        let (target, cdp) = self.current()?;
        self.ensure_ready(&target, &cdp).await?;
        self.remove_probes().await;
        let clock = self.align_clock(&target, &cdp).await?;
        let t = Thresholds {
            long_frame_ms,
            over_ms,
            ..Thresholds::default()
        };
        let id = crate::home::token();
        *self.measuring.lock().unwrap() = Some(Measuring {
            id: id.clone(),
            target: target.clone(),
            origin: 0.0,
            long_frame_ms,
            over_ms,
            key_downs: Vec::new(),
        });
        let installed = evaluate(&cdp, &measure::install_script(&id, &t, watch.as_ref())).await;
        let origin = match installed {
            Ok(v) if v["missing"].is_null() => v["origin"].as_f64().unwrap_or(0.0),
            Ok(v) => {
                self.remove_probes().await;
                let missing = v["missing"].as_str().unwrap_or("");
                return Err(Failure::new(
                    Kind::NotFound,
                    format!("no element matches `{missing}` to watch"),
                )
                .with("selector", missing));
            }
            Err(f) => {
                self.remove_probes().await;
                return Err(f);
            }
        };
        if let Some(m) = self.measuring.lock().unwrap().as_mut() {
            if m.id == id {
                m.origin = origin;
            }
        }
        Ok(fields! {
            "target" => target,
            "time_origin" => origin,
            "watch" => watch.as_ref().map(|w| json!({ "selector": w.selector, "attr": w.attr })),
            "clock" => clock.to_json(),
        })
    }

    pub(super) async fn measure_stop_cmd(&self, quiet_ms: u64, ceiling_ms: u64) -> Outcome {
        let Some(m) = self.measuring.lock().unwrap().clone() else {
            return Err(Failure::new(
                Kind::Error,
                "no measurement is running; run `measure start` first",
            ));
        };
        let cdp = self.cdp(&m.target)?;
        let t = Thresholds {
            long_frame_ms: m.long_frame_ms,
            over_ms: m.over_ms,
            quiet_ms,
            ceiling_ms,
        };
        let read = async {
            let begun = Instant::now();
            let ceiling = Duration::from_millis(ceiling_ms);
            let settled = loop {
                let s = page_state(&cdp, &m).await?;
                if s["quiet_ms"].as_f64().unwrap_or(0.0) >= quiet_ms as f64 {
                    break true;
                }
                if begun.elapsed() >= ceiling {
                    break false;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            };
            let waited = begun.elapsed().as_millis() as u64;
            let raw = evaluate(&cdp, &measure::read_script(&m.id)).await?;
            if raw.is_null() {
                return Err(reloaded(&m, None));
            }
            Ok::<_, Failure>((settled, waited, raw))
        };
        let read = read.await;
        self.remove_measurement(&m.id).await;
        let (settled, waited, raw) = read?;
        let mut f = measure::summarize(&raw, &t);
        f.insert("settled".into(), json!(settled));
        f.insert("settle_wait_ms".into(), json!(waited));
        f.insert("target".into(), json!(m.target));
        let uncertainty = self
            .clock
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0.0, |(_, c)| clock::round3(c.uncertainty_ms));
        f.insert(
            "latency".into(),
            measure::latency(&raw, &m.key_downs, uncertainty),
        );
        f.insert("clock".into(), self.clock_json());
        Ok(f)
    }

    /// Removes the probes of the running measurement, if any.
    pub(super) async fn remove_probes(&self) {
        let id = self
            .measuring
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.id.clone());
        if let Some(id) = id {
            self.remove_measurement(&id).await;
        }
    }

    /// Ends measurement `id`: forgets it unless a newer one has replaced it, and removes its
    /// probes from the page.
    async fn remove_measurement(&self, id: &str) {
        let m = {
            let mut measuring = self.measuring.lock().unwrap();
            match measuring.as_ref() {
                Some(m) if m.id == id => measuring.take(),
                _ => None,
            }
        };
        let target = m.map(|m| m.target);
        if let Some(Ok(cdp)) = target.map(|t| self.cdp(&t)) {
            let _ = evaluate_within(&cdp, &measure::remove_script(id), REMOVE_WAIT).await;
        }
    }
}

/// The page's state for measurement `m`, failing the run when its document has changed. A
/// failed evaluation is retried once, because a reload can destroy the context mid-call.
async fn page_state(cdp: &Cdp, m: &Measuring) -> Result<Value, Failure> {
    let script = measure::state_script(&m.id);
    let s = match evaluate(cdp, &script).await {
        Ok(s) => s,
        Err(first) => {
            tokio::time::sleep(Duration::from_millis(300)).await;
            evaluate(cdp, &script).await.map_err(|_| first)?
        }
    };
    let origin = s["origin"].as_f64();
    if origin == Some(m.origin) && s["installed"] == true {
        Ok(s)
    } else {
        Err(reloaded(m, origin))
    }
}

fn reloaded(m: &Measuring, origin_now: Option<f64>) -> Failure {
    Failure::new(
        Kind::Reloaded,
        "the page reloaded between `measure start` and `measure stop`; this run is void",
    )
    .with("time_origin_at_start", m.origin)
    .with("time_origin_now", origin_now)
}
