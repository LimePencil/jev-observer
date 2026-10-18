//! A dbus proxy for speaking with secret service's `Prompt` Interface.

use zbus::Variant;

/// A dbus proxy for speaking with secret service's `Prompt` Interface.
///
/// This will derive PromptProxy
///
/// Note that `Variant` in the method signatures corresponds to `VARIANT` dbus type.
#[zbus::proxy(
    interface = "org.freedesktop.Secret.Prompt",
    default_service = "org.freedesktop.Secret.Prompt"
)]
pub trait Prompt {
    fn prompt(&self, window_id: &str) -> zbus::Result<()>;

    fn dismiss(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn completed(&self, dismissed: bool, result: Variant<'_>) -> zbus::Result<()>;
}
