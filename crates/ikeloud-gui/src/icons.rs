//! The saxophone icon with a status badge, drawn for this project.
//!
//! One SVG per state. They are written to a folder at start (the tray host
//! loads them by name through `IconThemePath`) and rendered to pixels for
//! hosts that only take `IconPixmap`.

use std::path::{Path, PathBuf};

use ikeloud_config::State;

/// The app icon name (also the application id).
pub const APP_ID: &str = "io.github.brbla.IkeLoud";

/// Look of the badge for one state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Badge {
    /// Up to date: green with a tick.
    Idle,
    /// Syncing: blue with arrows.
    Syncing,
    /// Needs the user (confirmation, sign-in): orange with "!".
    Attention,
    /// Error: red with "!".
    Error,
    /// Offline, paused, starting: grey with a bar.
    Quiet,
}

impl Badge {
    /// Badge for a daemon state; `paused` wins.
    pub fn for_state(state: Option<State>, paused: bool) -> Self {
        if paused {
            return Self::Quiet;
        }
        match state {
            Some(State::Idle) => Self::Idle,
            Some(State::Syncing) => Self::Syncing,
            Some(State::NeedsConfirmation | State::SignedOut) => Self::Attention,
            Some(State::Error) => Self::Error,
            Some(State::Offline | State::Starting) | None => Self::Quiet,
        }
    }

    /// Icon name of this badge.
    pub fn icon_name(self) -> String {
        let suffix = match self {
            Self::Idle => "idle",
            Self::Syncing => "syncing",
            Self::Attention => "attention",
            Self::Error => "error",
            Self::Quiet => "quiet",
        };
        format!("{APP_ID}-{suffix}")
    }

    /// All badges.
    pub const ALL: [Self; 5] = [
        Self::Idle,
        Self::Syncing,
        Self::Attention,
        Self::Error,
        Self::Quiet,
    ];

    fn color(self) -> &'static str {
        match self {
            Self::Idle => "#26a269",
            Self::Syncing => "#1c71d8",
            Self::Attention => "#e66100",
            Self::Error => "#c01c28",
            Self::Quiet => "#77767b",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Idle => {
                r##"<path d="M9 50 l3.5 3.5 l6.5 -7" fill="none" stroke="#fff" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>"##
            }
            Self::Syncing => {
                r##"<path d="M9.5 48.5 a5 5 0 0 1 9 -1.5 M18.5 51.5 a5 5 0 0 1 -9 1.5" fill="none" stroke="#fff" stroke-width="2.2" stroke-linecap="round"/><path d="M19.5 44.5 v3.5 h-3.5 M8.5 55.5 v-3.5 h3.5" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>"##
            }
            Self::Attention | Self::Error => {
                r##"<path d="M14 44.5 v6" stroke="#fff" stroke-width="2.8" stroke-linecap="round"/><circle cx="14" cy="55" r="1.6" fill="#fff"/>"##
            }
            Self::Quiet => {
                r##"<path d="M9.5 50 h9" stroke="#fff" stroke-width="2.8" stroke-linecap="round"/>"##
            }
        }
    }

    /// The SVG.
    pub fn svg(self) -> String {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">
  <!-- mouthpiece and neck -->
  <path d="M15 5 L22 8" stroke="#3d3846" stroke-width="4" stroke-linecap="round"/>
  <path d="M21 7.5 Q29 8 30 15" fill="none" stroke="#c88800" stroke-width="4" stroke-linecap="round"/>
  <!-- body bending into the bell -->
  <path d="M30 14 L30 41 Q30 55 41 55 Q51 55 51 44 L51 37" fill="none" stroke="#e5a50a" stroke-width="8" stroke-linecap="round"/>
  <path d="M43.5 39 L42 28 Q51 25 60 28 L58.5 39 Z" fill="#e5a50a" stroke="#c88800" stroke-width="1.5" stroke-linejoin="round"/>
  <ellipse cx="51" cy="28" rx="9" ry="2.2" fill="#9c6e03"/>
  <!-- keys -->
  <circle cx="30" cy="21" r="2.4" fill="#9c6e03"/>
  <circle cx="30" cy="28" r="2.4" fill="#9c6e03"/>
  <circle cx="30" cy="35" r="2.4" fill="#9c6e03"/>
  <circle cx="36" cy="48" r="2" fill="#9c6e03"/>
  <!-- status badge -->
  <circle cx="14" cy="50" r="10.5" fill="{color}" stroke="#ffffff" stroke-width="2"/>
  {glyph}
</svg>
"##,
            color = self.color(),
            glyph = self.glyph()
        )
    }
}

/// The app icon: the saxophone without a status badge.
pub fn app_svg() -> String {
    let svg = Badge::Quiet.svg();
    match (svg.find("  <!-- status badge -->"), svg.rfind("</svg>")) {
        (Some(start), Some(end)) if start < end => {
            format!(
                "{}{}",
                svg.get(..start).unwrap_or_default(),
                svg.get(end..).unwrap_or_default()
            )
        }
        _ => svg,
    }
}

/// Puts the app icon into the user's icon theme folder
/// (`$XDG_DATA_HOME/icons/hicolor/scalable/apps`), where menus and the
/// desktop file find it. Kept up to date on every start.
pub fn install_app_icon(data_dir: &Path) -> std::io::Result<()> {
    let dir = data_dir
        .join("icons")
        .join("hicolor")
        .join("scalable")
        .join("apps");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{APP_ID}.svg"));
    let svg = app_svg();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(svg.as_str()) {
        std::fs::write(path, svg)?;
    }
    Ok(())
}

/// Writes all icons under `dir` (flat, and as a hicolor theme, so either
/// lookup finds them). Returns the folder to give as `IconThemePath`.
pub fn install(dir: &Path) -> std::io::Result<PathBuf> {
    let themed = dir.join("hicolor").join("scalable").join("apps");
    std::fs::create_dir_all(&themed)?;
    for badge in Badge::ALL {
        let name = format!("{}.svg", badge.icon_name());
        let svg = badge.svg();
        std::fs::write(dir.join(&name), &svg)?;
        std::fs::write(themed.join(&name), &svg)?;
    }
    // The app icon (window, launchers): the saxophone without a badge.
    std::fs::write(themed.join(format!("{APP_ID}.svg")), app_svg())?;
    std::fs::write(dir.join(format!("{APP_ID}.svg")), app_svg())?;
    Ok(dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_has_a_badge() {
        assert_eq!(Badge::for_state(Some(State::Idle), false), Badge::Idle);
        assert_eq!(Badge::for_state(Some(State::Idle), true), Badge::Quiet);
        assert_eq!(
            Badge::for_state(Some(State::SignedOut), false),
            Badge::Attention
        );
        assert_eq!(Badge::for_state(Some(State::Error), false), Badge::Error);
        assert_eq!(Badge::for_state(None, false), Badge::Quiet);
        for badge in Badge::ALL {
            let svg = badge.svg();
            assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
            assert!(badge.icon_name().starts_with(APP_ID));
        }
    }

    #[test]
    fn packaged_icon_is_the_app_icon() {
        // packaging/icons/ is what the install script and packages install
        // for menus before the app ever ran; it must be the app's icon.
        let packaged = include_str!("../../../packaging/icons/io.github.brbla.IkeLoud.svg");
        assert_eq!(
            packaged,
            app_svg(),
            "regenerate packaging/icons from app_svg()"
        );
    }
}
