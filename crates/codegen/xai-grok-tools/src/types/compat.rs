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

    #[test]
    fn skill_config_dirs_all_on_matches_legacy_constant() {
        // Historical constant was `[".grok", ".agents", ".claude", ".cursor"]`.
        assert_eq!(
            CompatConfig::default().skill_config_dirs(),
            vec![".grok", ".agents", ".claude", ".cursor"]
        );
    }

    #[test]
    fn skill_config_dirs_gates_each_vendor() {
        let mut c = CompatConfig::default();
        c.cursor.skills = false;
        assert_eq!(c.skill_config_dirs(), vec![".grok", ".agents", ".claude"]);

        c.claude.skills = false;
        assert_eq!(c.skill_config_dirs(), vec![".grok", ".agents"]);

        // Only the `cursor` cell on (`claude` off): `cursor` still appended last.
        let mut c2 = CompatConfig::default();
        c2.claude.skills = false;
        assert_eq!(c2.skill_config_dirs(), vec![".grok", ".agents", ".cursor"]);
    }

    #[test]
    fn rules_dirs_all_on_matches_legacy_constant() {
        // Historical `RULES_DIRS` was `[".grok/rules", ".claude/rules", ".cursor/rules"]`.
        assert_eq!(
            CompatConfig::default().rules_dirs(),
            vec![".grok/rules", ".claude/rules", ".cursor/rules"]
        );
    }

    #[test]
    fn rules_dirs_gates_each_vendor() {
        let mut c = CompatConfig::default();
        c.cursor.rules = false;
        assert_eq!(c.rules_dirs(), vec![".grok/rules", ".claude/rules"]);
        c.claude.rules = false;
        assert_eq!(c.rules_dirs(), vec![".grok/rules"]);
    }

    #[test]
    fn agent_filenames_all_on_matches_legacy_constant() {
        // Historical `AGENT_FILENAMES`.
        assert_eq!(
            CompatConfig::default().agent_filenames(),
            vec![
                "Agents.md",
                "Claude.md",
                "CLAUDE.md",
                "CLAUDE.local.md",
                "AGENT.md",
                "AGENTS.md",
                ".claude/CLAUDE.md",
                ".claude/CLAUDE.local.md",
            ]
        );
    }

    #[test]
    fn agent_filenames_drops_claude_subdir_when_off() {
        let mut c = CompatConfig::default();
        c.claude.agents = false;
        assert_eq!(
            c.agent_filenames(),
            vec![
                "Agents.md",
                "Claude.md",
                "CLAUDE.md",
                "CLAUDE.local.md",
                "AGENT.md",
                "AGENTS.md",
            ]
        );
    }

    #[test]
    fn agents_home_dirs_all_on_matches_legacy_constant() {
        // Historical home scan was `[".claude", ".cursor"]`.
        assert_eq!(
            CompatConfig::default().agents_home_dirs(),
            vec![".claude", ".cursor"]
        );
    }

    #[test]
    fn agents_home_dirs_gates_each_vendor() {
        let mut c = CompatConfig::default();
        c.claude.agents = false;
        assert_eq!(c.agents_home_dirs(), vec![".cursor"]);
        c.cursor.agents = false;
        assert!(c.agents_home_dirs().is_empty());
    }

    #[test]
    fn toml_struct_deserializes_partial_cells() {
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

    #[test]
    fn skill_config_dirs_all_on_matches_legacy_constant() {
        // Historical constant was `[".grok", ".agents", ".claude", ".cursor"]`.
        assert_eq!(
            CompatConfig::default().skill_config_dirs(),
            vec![".grok", ".agents", ".claude", ".cursor"]
        );
    }

    #[test]
    fn skill_config_dirs_gates_each_vendor() {
        let mut c = CompatConfig::default();
        c.cursor.skills = false;
        assert_eq!(c.skill_config_dirs(), vec![".grok", ".agents", ".claude"]);

        c.claude.skills = false;
        assert_eq!(c.skill_config_dirs(), vec![".grok", ".agents"]);

        // Only the `cursor` cell on (`claude` off): `cursor` still appended last.
        let mut c2 = CompatConfig::default();
        c2.claude.skills = false;
        assert_eq!(c2.skill_config_dirs(), vec![".grok", ".agents", ".cursor"]);
    }

    #[test]
    fn rules_dirs_all_on_matches_legacy_constant() {
        // Historical `RULES_DIRS` was `[".grok/rules", ".claude/rules", ".cursor/rules"]`.
        assert_eq!(
            CompatConfig::default().rules_dirs(),
            vec![".grok/rules", ".claude/rules", ".cursor/rules"]
        );
    }

    #[test]
    fn rules_dirs_gates_each_vendor() {
        let mut c = CompatConfig::default();
        c.cursor.rules = false;
        assert_eq!(c.rules_dirs(), vec![".grok/rules", ".claude/rules"]);
        c.claude.rules = false;
        assert_eq!(c.rules_dirs(), vec![".grok/rules"]);
    }

    #[test]
    fn agent_filenames_all_on_matches_legacy_constant() {
        // Historical `AGENT_FILENAMES`.
        assert_eq!(
            CompatConfig::default().agent_filenames(),
            vec![
                "Agents.md",
                "Claude.md",
                "CLAUDE.md",
                "CLAUDE.local.md",
                "AGENT.md",
                "AGENTS.md",
                ".claude/CLAUDE.md",
                ".claude/CLAUDE.local.md",
            ]
        );
    }

    #[test]
    fn agent_filenames_drops_claude_subdir_when_off() {
        let mut c = CompatConfig::default();
        c.claude.agents = false;
        assert_eq!(
            c.agent_filenames(),
            vec![
                "Agents.md",
                "Claude.md",
                "CLAUDE.md",
                "CLAUDE.local.md",
                "AGENT.md",
                "AGENTS.md",
            ]
        );
    }

    #[test]
    fn agents_home_dirs_all_on_matches_legacy_constant() {
        // Historical home scan was `[".claude", ".cursor"]`.
        assert_eq!(
            CompatConfig::default().agents_home_dirs(),
            vec![".claude", ".cursor"]
        );
    }

    #[test]
    fn agents_home_dirs_gates_each_vendor() {
        let mut c = CompatConfig::default();
        c.claude.agents = false;
        assert_eq!(c.agents_home_dirs(), vec![".cursor"]);
        c.cursor.agents = false;
        assert!(c.agents_home_dirs().is_empty());
    }

    #[test]
    fn toml_struct_deserializes_partial_cells() {
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
