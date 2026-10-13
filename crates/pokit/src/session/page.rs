//! The page a command runs in: the engine behind the session, CDP or the app's plugin.

use super::commands::evaluate_within;
use super::State;
use crate::cdp::Cdp;
use crate::output::Failure;
use crate::plugin::Plugin;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// The app's plugin, and the webview commands go to.
pub(super) struct PluginPage {
    pub plugin: Plugin,
    pub webview: String,
}

/// One page, on whichever engine reaches it.
pub(super) enum Page<'a> {
    Cdp(Arc<Cdp>),
    Plugin(&'a PluginPage),
}

impl Page<'_> {
    /// Evaluates `js`, awaiting a promise; the value comes back as JSON.
    pub async fn eval(&self, js: &str, timeout: Duration) -> Result<Value, Failure> {
        match self {
            Page::Cdp(cdp) => evaluate_within(cdp, js, timeout).await,
            Page::Plugin(p) => p.plugin.eval(&p.webview, js, timeout).await,
        }
    }
}

impl State {
    /// The page commands go to, and its id: the current target over CDP, the main webview
    /// through the plugin.
    pub(super) fn page(&self) -> Result<(String, Page<'_>), Failure> {
        match &self.plugin {
            Some(p) => Ok((p.webview.clone(), Page::Plugin(p))),
            None => self.current().map(|(id, cdp)| (id, Page::Cdp(cdp))),
        }
    }
}
