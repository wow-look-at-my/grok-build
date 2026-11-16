//! Vendor compatibility configuration; `xai-grok-config` owns it.

<<<<<<< fork
}

pub const COMPAT_CELLS: [CompatCell; 12] = [
];

/// Raw per-vendor compat cells parsed from `[compat.<vendor>]` TOML.
///
/// Resolution order is env override, this value, remote flag, default ON.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct CompatConfigToml {
    #[serde(default)]
    pub cursor: VendorCompatToml,
    #[serde(default)]
    pub claude: VendorCompatToml,
}

impl CompatConfigToml {
    pub fn value(&self, cell: CompatCell) -> Option<bool> {
        match cell.vendor() {
            CompatVendor::Cursor => self.cursor.value(cell.surface()),
            CompatVendor::Claude => self.claude.value(cell.surface()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompatConfig {
    pub cursor: VendorCompat,
    pub claude: VendorCompat,
}

impl CompatConfig {
    pub fn value(&self, cell: CompatCell) -> bool {
        match cell.vendor() {
            CompatVendor::Cursor => self.cursor.value(cell.surface()),
            CompatVendor::Claude => self.claude.value(cell.surface()),
        }
    }

    pub fn set(&mut self, cell: CompatCell, value: bool) {
        match cell.vendor() {
            CompatVendor::Cursor => self.cursor.set(cell.surface(), value),
            CompatVendor::Claude => self.claude.set(cell.surface(), value),
        }
    }

            ]
        );

        let defaults = CompatConfig::default();
        for cell in COMPAT_CELLS {
            assert!(
                defaults.value(cell),
                "{}.{}",
                Into::<&'static str>::into(cell.vendor()),
                Into::<&'static str>::into(cell.surface())
            );
        }
        for vendor in [defaults.cursor, defaults.claude] {
            assert!(vendor.skills && vendor.rules && vendor.agents);
            assert!(vendor.mcps && vendor.hooks);
            assert!(vendor.sessions);
        }
    }

        // The raw TOML struct is parsed from `[compat]` in the shell crate
        // (where `toml` is a dep).
        let parsed: CompatConfigToml =
            serde_yaml::from_str("cursor:\n  skills: false\n  sessions: true\n").unwrap();
        assert_eq!(parsed.cursor.skills, Some(false));
        assert_eq!(parsed.cursor.rules, None);
        assert_eq!(parsed.cursor.sessions, Some(true));
        assert_eq!(parsed.claude, VendorCompatToml::default());

        // mcps cell round-trips the same way.
        let parsed: CompatConfigToml = serde_yaml::from_str("claude:\n  mcps: false\n").unwrap();
        assert_eq!(parsed.claude.mcps, Some(false));
        assert_eq!(parsed.claude.hooks, None);
        assert_eq!(parsed.claude.sessions, None);
        assert_eq!(parsed.cursor, VendorCompatToml::default());
    }
}
||||||| base
    Codex,
    CodexSessions,

    /// Whether Grok currently implements this compatibility surface. Codex non-session cells remain
    /// reserved in the registry so their config shape is stable, but runtime discovery does not
    /// consume them.
    pub const fn is_runtime_supported(self) -> bool {
        match self.vendor {
            CompatVendor::Cursor | CompatVendor::Claude => true,
            CompatVendor::Codex => matches!(self.surface, CompatSurface::Sessions),
        }
    }
}

pub const COMPAT_CELLS: [CompatCell; 18] = [
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Skills,
        "GROK_CODEX_SKILLS_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Rules,
        "GROK_CODEX_RULES_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Agents,
        "GROK_CODEX_AGENTS_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Mcps,
        "GROK_CODEX_MCPS_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Hooks,
        "GROK_CODEX_HOOKS_ENABLED",
        None,
    ),
    CompatCell::new(
        CompatVendor::Codex,
        CompatSurface::Sessions,
        "GROK_CODEX_SESSIONS_ENABLED",
        Some(CompatRemoteKey::CodexSessions),
    ),
];

/// Raw per-vendor compat cells parsed from `[compat.<vendor>]` TOML.
///
/// Resolution order is env override, this value, remote flag, default ON.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CompatConfigToml {
    #[serde(default)]
    pub cursor: VendorCompatToml,
    #[serde(default)]
    pub claude: VendorCompatToml,
    #[serde(default)]
    pub codex: VendorCompatToml,
}

impl CompatConfigToml {
    pub fn value(&self, cell: CompatCell) -> Option<bool> {
        match cell.vendor() {
            CompatVendor::Cursor => self.cursor.value(cell.surface()),
            CompatVendor::Claude => self.claude.value(cell.surface()),
            CompatVendor::Codex => self.codex.value(cell.surface()),
        }
    }
}

/// Codex's non-session cells are reserved and are not consumed by discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompatConfig {
    pub cursor: VendorCompat,
    pub claude: VendorCompat,
    pub codex: VendorCompat,
}

impl CompatConfig {
    pub fn value(&self, cell: CompatCell) -> bool {
        match cell.vendor() {
            CompatVendor::Cursor => self.cursor.value(cell.surface()),
            CompatVendor::Claude => self.claude.value(cell.surface()),
            CompatVendor::Codex => self.codex.value(cell.surface()),
        }
    }

    pub fn set(&mut self, cell: CompatCell, value: bool) {
        match cell.vendor() {
            CompatVendor::Cursor => self.cursor.set(cell.surface(), value),
            CompatVendor::Claude => self.claude.set(cell.surface(), value),
            CompatVendor::Codex => self.codex.set(cell.surface(), value),
        }
    }

                ("codex", "skills", None),
                ("codex", "rules", None),
                ("codex", "agents", None),
                ("codex", "mcps", None),
                ("codex", "hooks", None),
                ("codex", "sessions", Some(CodexSessions)),
            ]
        );

        let defaults = CompatConfig::default();
        for cell in COMPAT_CELLS {
            assert!(
                defaults.value(cell),
                "{}.{}",
                Into::<&'static str>::into(cell.vendor()),
                Into::<&'static str>::into(cell.surface())
            );
        }
        for vendor in [defaults.cursor, defaults.claude, defaults.codex] {
            assert!(vendor.skills && vendor.rules && vendor.agents);
            assert!(vendor.mcps && vendor.hooks);
            assert!(vendor.sessions);
        }

        assert_eq!(
            COMPAT_CELLS
                .into_iter()
                .filter(|cell| cell.is_runtime_supported())
                .map(|cell| {
                    (
                        Into::<&'static str>::into(cell.vendor()),
                        Into::<&'static str>::into(cell.surface()),
                    )
                })
                .collect::<Vec<_>>(),
            [
                ("cursor", "skills"),
                ("cursor", "rules"),
                ("cursor", "agents"),
                ("cursor", "mcps"),
                ("cursor", "hooks"),
                ("cursor", "sessions"),
                ("claude", "skills"),
                ("claude", "rules"),
                ("claude", "agents"),
                ("claude", "mcps"),
                ("claude", "hooks"),
                ("claude", "sessions"),
                ("codex", "sessions"),
            ]
        );
    }

        // The raw TOML struct is parsed from `[compat]` in the shell crate (where `toml` is a dep). Here we exercise the same
        // serde shape via YAML (available in this crate) to pin the `Option<bool>` + `#[serde(default)]` semantics: unset
        // cells stay `None`, unset vendors default-construct.
        let parsed: CompatConfigToml = serde_yaml::from_str(
            "cursor:\n  skills: false\n  sessions: true\ncodex:\n  sessions: true\n",
        )
        .unwrap();
        assert_eq!(parsed.cursor.skills, Some(false));
        assert_eq!(parsed.cursor.rules, None);
        assert_eq!(parsed.cursor.sessions, Some(true));
        assert_eq!(parsed.claude, VendorCompatToml::default());
        assert_eq!(parsed.codex.sessions, Some(true));
        assert_eq!(parsed.codex.skills, None);

        // mcps cell round-trips the same way.
        let parsed: CompatConfigToml = serde_yaml::from_str("claude:\n  mcps: false\n").unwrap();
        assert_eq!(parsed.claude.mcps, Some(false));
        assert_eq!(parsed.claude.hooks, None);
        assert_eq!(parsed.claude.sessions, None);
        assert_eq!(parsed.cursor, VendorCompatToml::default());
        assert_eq!(parsed.codex, VendorCompatToml::default());
    }
}
=======
pub(crate) use xai_grok_config::compat::INSTRUCTION_FILENAMES;
pub use xai_grok_config::compat::{
    COMPAT_CELLS, CompatCell, CompatConfig, CompatConfigToml, CompatRemoteKey, CompatSurface,
    CompatVendor, VendorCompat, VendorCompatToml,
};
>>>>>>> upstream
