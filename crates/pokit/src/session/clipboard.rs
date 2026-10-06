//! `clipboard read` / `clipboard write`, and giving the user's clipboard back when the session ends.

use super::State;
#[cfg(windows)]
use crate::fields;
#[cfg(windows)]
use crate::output::{Failure, Kind, Outcome};

/// The window that owns pokit's clipboard writes, started at the first clipboard command.
#[derive(Default)]
pub(super) struct ClipboardState {
    #[cfg(windows)]
    owner: Option<std::sync::Arc<crate::clipboard::Clipboard>>,
}

impl State {
    #[cfg(windows)]
    pub(super) async fn clipboard_read_cmd(&self) -> Outcome {
        let owner = self.clipboard_owner()?;
        Ok(match blocking(move || owner.read_text()).await? {
            crate::clipboard::Read::Text(text) => fields! { "text" => text },
            crate::clipboard::Read::Withheld => fields! {
                "text" => serde_json::Value::Null,
                "withheld" => "another program marked the clipboard as not for monitoring                                (ExcludeClipboardContentFromMonitorProcessing), as password managers do",
            },
        })
    }

    #[cfg(windows)]
    pub(super) async fn clipboard_write_cmd(&self, text: &str) -> Outcome {
        let owner = self.clipboard_owner()?;
        let chars = text.chars().count();
        let text = text.to_string();
        let saved = blocking(move || owner.write_text(&text)).await?;
        Ok(fields! { "written_chars" => chars, "saved_user_clipboard" => saved })
    }

    /// Puts back what the clipboard held before the session's first write, unless something
    /// other than pokit has written to it since.
    pub(super) async fn restore_clipboard(&self) {
        #[cfg(windows)]
        {
            let owner = self.clipboard.lock().unwrap().owner.clone();
            let Some(owner) = owner else {
                return;
            };
            match blocking(move || owner.restore()).await {
                Ok(true) => {}
                Ok(false) => self.log(
                    "session",
                    "info",
                    "",
                    "the clipboard was not given back: pokit never wrote to it, or something else has since"
                        .into(),
                ),
                Err(e) => self.log(
                    "session",
                    "warning",
                    "",
                    format!("could not give the clipboard back fully: {}", e.message),
                ),
            }
        }
    }

    #[cfg(windows)]
    fn clipboard_owner(&self) -> Result<std::sync::Arc<crate::clipboard::Clipboard>, Failure> {
        let mut c = self.clipboard.lock().unwrap();
        if let Some(owner) = &c.owner {
            return Ok(owner.clone());
        }
        let owner =
            std::sync::Arc::new(crate::clipboard::Clipboard::start().map_err(Failure::from)?);
        c.owner = Some(owner.clone());
        Ok(owner)
    }
}

/// Runs a blocking clipboard call off the async runtime.
#[cfg(windows)]
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Failure::new(Kind::Error, format!("the clipboard call failed: {e}")))?
        .map_err(Failure::from)
}
