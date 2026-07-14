#![forbid(unsafe_code)]
//! Backend-neutral domain values and behavior for Tracetide.

/// Reserved identity of an immutable built-in profile.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActiveProfileId {
    /// Fallout 4 built-in profile.
    Fo4,
    /// Skyrim Special Edition built-in profile and fresh-install default.
    #[default]
    Sse,
    /// Classic Skyrim built-in profile.
    Tes5,
}

/// Mutable processing choices applied over the active profile definition.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProfileOverlay {
    dry_run: bool,
}

impl ProfileOverlay {
    /// Returns whether processing should report writes without committing them.
    #[must_use]
    pub const fn dry_run(self) -> bool {
        self.dry_run
    }

    /// Returns an owned overlay with the requested dry-run choice.
    #[must_use]
    pub const fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }
}

/// Persisted setup values that are authoritative before a processing run begins.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetupState {
    active_profile: ActiveProfileId,
    profile_overlay: ProfileOverlay,
}

impl SetupState {
    /// Returns the stable identity of the profile governing setup.
    #[must_use]
    pub const fn active_profile(&self) -> ActiveProfileId {
        self.active_profile
    }

    /// Returns the processing choices for the active profile.
    #[must_use]
    pub const fn profile_overlay(&self) -> ProfileOverlay {
        self.profile_overlay
    }

    /// Returns an owned setup value with the supplied profile overlay.
    #[must_use]
    pub fn with_profile_overlay(&self, profile_overlay: ProfileOverlay) -> Self {
        let mut setup = self.clone();
        setup.profile_overlay = profile_overlay;
        setup
    }
}
