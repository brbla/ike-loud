//! Converting types from other crates into messages with keys.
//!
//! Every variant has its own key; the test `every_message_is_translated`
//! checks that the key exists in every built-in language.

use ikeloud_safety::names::NameError;
use ikeloud_safety::{DeferReason, ReadOnlyReason, Rejection};

use crate::{Message, Value};

/// A type that knows which message shows it to the user.
pub trait ToMessage {
    /// Message for the user.
    fn message(&self) -> Message;
}

impl ToMessage for Rejection {
    fn message(&self) -> Message {
        match self {
            Self::LocalFolderUnavailable => Message::new("rejected-local-folder-unavailable"),
            Self::SuspiciousRemoteListing => Message::new("rejected-suspicious-remote-listing"),
            Self::Conflict(paths) => {
                Message::new("rejected-conflict").arg("count", Value::Number(paths.len() as u64))
            }
            Self::MassChangeNeedsConfirmation(summary) => Message::new("rejected-mass-change")
                .arg("count", Value::Number(summary.count()))
                .arg("total", Value::Number(summary.total)),
            Self::NotEnoughICloudSpace { needed, free } => Message::new("rejected-icloud-space")
                .arg("needed", Value::Size(*needed))
                .arg("free", Value::Size(*free)),
            Self::NotEnoughDiskSpace { needed, free } => Message::new("rejected-disk-space")
                .arg("needed", Value::Size(*needed))
                .arg("free", Value::Size(*free)),
        }
    }
}

impl ToMessage for DeferReason {
    fn message(&self) -> Message {
        match self {
            Self::ReadOnly(reason) => Message::new("deferred-read-only")
                .arg("reason", Value::Message(Box::new(reason.message()))),
            Self::StillBeingWritten => Message::new("deferred-still-being-written"),
            Self::InvalidName(error) => error.message(),
            Self::Ignored => Message::new("deferred-ignored"),
            Self::CaseCollision { with } => Message::new("deferred-case-collision")
                .arg("name", Value::Text(with.name().to_string())),
        }
    }
}

impl ToMessage for NameError {
    fn message(&self) -> Message {
        match self {
            Self::EmptyComponent => Message::new("deferred-name-empty"),
            Self::DotOrDotDot => Message::new("deferred-name-dot"),
            Self::ForbiddenChar(_) => Message::new("deferred-name-char"),
            Self::TooLong => Message::new("deferred-name-too-long"),
        }
    }
}

impl ToMessage for ReadOnlyReason {
    fn message(&self) -> Message {
        Message::new(match self {
            Self::Settings => "read-only-settings",
            Self::FirstDownload => "read-only-first-download",
            Self::UnknownApiResponse => "read-only-unknown-api",
            Self::RemoteKillSwitch => "read-only-kill-switch",
        })
    }
}
