use super::*;

fn team(id: &str) -> ServingIdentity {
    ServingIdentity::Team(id.to_owned())
}

/// The authoritative signed verdict wins over the marker BOTH ways.
/// `Compromised` refuses where the marker alone would pass; `Trusted` proceeds over marker tamper.
#[test]
fn signed_verdict_overrides_marker_both_ways() {
    use crate::signed_policy::SignedVerdict;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // Marker: opted-in, served requirements now MISSING on disk (marker alone would refuse).
    let cache = ManagedConfigCache {
        principal: Some("team-007".into()),
        had_requirements: true,
        fail_closed: true,
        ..Default::default()
    };
    // Signed says NOT compromised, so it proceeds, overriding the marker's tamper signal
    assert!(!managed_policy_compromised_decision(
        SignedVerdict::Trusted,
        || false,
        Some(&cache),
        home,
        &team("team-007")
    ));
    // Signed says compromised, so it refuses, though this intact marker alone would pass
    let intact = ManagedConfigCache {
        principal: Some("team-007".into()),
        fail_closed: true,
        ..Default::default()
    };
    assert!(managed_policy_compromised_decision(
        SignedVerdict::Compromised,
        || false,
        Some(&intact),
        home,
        &team("team-007")
    ));
}

/// A sidecar read BLIP is not absence: unlike NoAuthenticSidecar it never refuses on its own; the marker decision stands.
#[test]
fn unreadable_sidecar_falls_back_to_marker() {
    use crate::signed_policy::SignedVerdict;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // Served artifact INTACT: the same marker refuses under NoAuthenticSidecar (pinned below) but must ALLOW under a mere read blip
    std::fs::write(home.join("requirements.toml"), "[features]\n").unwrap();
    let served_fail_closed = ManagedConfigCache {
        principal: Some("team-007".into()),
        had_requirements: true,
        fail_closed: true,
        ..Default::default()
    };
    assert!(
        !managed_policy_compromised_decision(
            SignedVerdict::SidecarUnreadable,
            || false,
            Some(&served_fail_closed),
            home,
            &team("team-007")
        ),
        "a transient sidecar read blip must not refuse a session"
    );
    // Marker-grade tamper (served artifact missing on disk) still refuses.
    std::fs::remove_file(home.join("requirements.toml")).unwrap();
    assert!(managed_policy_compromised_decision(
        SignedVerdict::SidecarUnreadable,
        || false,
        Some(&served_fail_closed),
        home,
        &team("team-007")
    ));
}

/// NoAuthenticSidecar under a fail-closed marker that recorded served policy refuses.
/// Stripping the sidecar must not downgrade enforcement to the forgeable marker path.
/// A marker that served nothing, never opted in, or is absent keeps the marker decision.
#[test]
fn missing_sidecar_under_fail_closed_marker_refuses() {
    use crate::signed_policy::SignedVerdict;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // The served artifact is INTACT on disk; the marker path alone would allow
    std::fs::write(home.join("requirements.toml"), "[features]\n").unwrap();
    let served_fail_closed = ManagedConfigCache {
        principal: Some("team-007".into()),
        had_requirements: true,
        fail_closed: true,
        ..Default::default()
    };
    assert!(
        managed_policy_compromised_decision(
            SignedVerdict::NoAuthenticSidecar,
            || false,
            Some(&served_fail_closed),
            home,
            &team("team-007")
        ),
        "a fail-closed marker with served policy requires an authentic sidecar"
    );
    // Served nothing leaves nothing the sidecar must cover, so the marker decision stands
    let served_nothing = ManagedConfigCache {
        principal: Some("team-007".into()),
        fail_closed: true,
        ..Default::default()
    };
    assert!(!managed_policy_compromised_decision(
        SignedVerdict::NoAuthenticSidecar,
        || false,
        Some(&served_nothing),
        home,
        &team("team-007")
    ));
    // Never opted in, so the marker decision stands
    let opted_out = ManagedConfigCache {
        principal: Some("team-007".into()),
        had_requirements: true,
        fail_closed: false,
        ..Default::default()
    };
    assert!(!managed_policy_compromised_decision(
        SignedVerdict::NoAuthenticSidecar,
        || false,
        Some(&opted_out),
        home,
        &team("team-007")
    ));
    // No marker at all means nothing to enforce
    assert!(!managed_policy_compromised_decision(
        SignedVerdict::NoAuthenticSidecar,
        || false,
        None,
        home,
        &team("team-007")
    ));
}

/// The dark build (`Inactive`) falls through to the best-effort marker.
/// Opted-in with a served artifact now missing refuses; opted-out or no marker proceeds.
#[test]
fn inactive_verdict_falls_through_to_marker() {
    use crate::signed_policy::SignedVerdict;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // Opted-in and the recorded requirements artifact is absent on disk, so the decision is compromised
    let missing = ManagedConfigCache {
        principal: Some("team-007".into()),
        had_requirements: true,
        fail_closed: true,
        ..Default::default()
    };
    assert!(managed_policy_compromised_decision(
        SignedVerdict::Inactive,
        || false,
        Some(&missing),
        home,
        &team("team-007")
    ));
    // An opted-OUT marker never refuses, even with a missing artifact
    let optout = ManagedConfigCache {
        principal: Some("team-007".into()),
        had_requirements: true,
        fail_closed: false,
        ..Default::default()
    };
    assert!(!managed_policy_compromised_decision(
        SignedVerdict::Inactive,
        || false,
        Some(&optout),
        home,
        &team("team-007")
    ));
    // No marker at all means nothing to enforce
    assert!(!managed_policy_compromised_decision(
        SignedVerdict::Inactive,
        || false,
        None,
        home,
        &team("team-007")
    ));
}

#[test]
fn managed_config_stale_at_is_false_without_user_home() {
    // No user home means nothing to refresh into, so it is not stale (prevents a perpetual sync loop)
    assert!(!managed_config_stale_at(None, &ServingIdentity::None));
}

#[test]
fn managed_config_stale_at_is_true_without_synced_marker() {
    let dir = std::env::temp_dir().join(format!("grok-stale-nomark-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _ = std::fs::remove_file(dir.join(MANAGED_CONFIG_CACHE_FILE));
    // No recorded sync (even if config files exist) reads stale
    assert!(managed_config_stale_at(Some(&dir), &ServingIdentity::None));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn managed_config_stale_at_is_false_after_fresh_sync() {
    let dir = std::env::temp_dir().join(format!("grok-stale-fresh-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: None,
            had_managed_config: false,
            had_requirements: false,
            fail_closed: false,
        },
    );
    // A just-recorded sync is within the default 30-minute threshold.
    assert!(!managed_config_stale_at(Some(&dir), &ServingIdentity::None));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn managed_config_stale_at_is_true_for_old_sync() {
    let dir = std::env::temp_dir().join(format!("grok-stale-old-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hour_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 60 * 60;
    std::fs::write(
        dir.join(MANAGED_CONFIG_CACHE_FILE),
        format!("{{\"synced_at\":{hour_ago}}}"),
    )
    .unwrap();
    // An hour-old sync exceeds the default 30-minute threshold.
    assert!(managed_config_stale_at(Some(&dir), &ServingIdentity::None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Served-then-deleted is stale; an armed build also treats unsigned policy on disk as stale.
#[test]
fn managed_config_stale_when_served_artifact_deleted() {
    let dir = std::env::temp_dir().join(format!("grok-stale-artgone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-1"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: false,
        },
    );
    std::fs::write(dir.join("requirements.toml"), "[features]\n").unwrap();
    let cache = read_managed_config_cache(&dir).unwrap();
    // Present is usable; deleted reads as tamper
    assert!(!cache_unusable_for(&cache, &dir, &team("team-1")));
    // When armed, unsigned policy is still hard-stale
    assert!(managed_config_stale_at(Some(&dir), &team("team-1")));
    std::fs::remove_file(dir.join("requirements.toml")).unwrap();
    assert!(cache_unusable_for(&cache, &dir, &team("team-1")));
    assert!(managed_config_stale_at(Some(&dir), &team("team-1")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Empty/whitespace placeholders are not served artifacts (write-deny first-run slots).
#[test]
fn cache_missing_required_artifact_treats_empty_placeholder_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let cache = ManagedConfigCache {
        had_requirements: true,
        had_managed_config: true,
        ..Default::default()
    };
    assert!(cache_missing_required_artifact(&cache, home));
    std::fs::write(home.join("requirements.toml"), "").unwrap();
    std::fs::write(home.join("managed_config.toml"), "  \n").unwrap();
    assert!(
        cache_missing_required_artifact(&cache, home),
        "empty/whitespace placeholders must not count as served artifacts"
    );
    std::fs::write(home.join("requirements.toml"), "[features]\n").unwrap();
    std::fs::write(home.join("managed_config.toml"), "model = \"x\"\n").unwrap();
    assert!(!cache_missing_required_artifact(&cache, home));
}

/// A config-less principal that served nothing is never misread as stale.
#[test]
fn managed_config_not_stale_when_nothing_served() {
    let dir = std::env::temp_dir().join(format!("grok-stale-noart-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-1"),
            had_managed_config: false,
            had_requirements: false,
            fail_closed: false,
        },
    );
    assert!(!managed_config_stale_at(Some(&dir), &team("team-1")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A cache fetched for a different principal is stale for the current one.
#[test]
fn managed_config_stale_on_identity_mismatch() {
    let dir = std::env::temp_dir().join(format!("grok-stale-ident-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: false,
            fail_closed: false,
        },
    );
    // Same identity is fresh; a different identity is stale
    assert!(!managed_config_stale_at(Some(&dir), &team("team-a")));
    assert!(managed_config_stale_at(Some(&dir), &team("team-b")));
    // Unknown current identity (None) never forces a refetch on identity.
    assert!(!managed_config_stale_at(Some(&dir), &ServingIdentity::None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A legacy marker (no `had_*`) is never flagged stale for a missing artifact.
#[test]
fn managed_config_legacy_marker_is_conservative() {
    let dir = std::env::temp_dir().join(format!("grok-stale-legacy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(
        dir.join(MANAGED_CONFIG_CACHE_FILE),
        format!("{{\"synced_at\":{now}}}"),
    )
    .unwrap();
    assert!(!managed_config_stale_at(Some(&dir), &ServingIdentity::None));
    // A legacy marker (no principal) reads stale once via identity mismatch, so it self-upgrades next sync.
    assert!(managed_config_stale_at(Some(&dir), &team("team-x")));
    assert!(is_managed_config_hard_stale_for_at(&dir, &team("team-x")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Hard-stale is a missing artifact or an identity mismatch; a fresh same-identity cache is usable.
#[test]
fn hard_stale_only_on_missing_or_identity() {
    let dir = std::env::temp_dir().join(format!("grok-hardstale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: false,
        },
    );
    std::fs::write(dir.join("requirements.toml"), "[features]\n").unwrap();
    let cache = read_managed_config_cache(&dir).unwrap();
    // Same identity with the artifact present is usable
    assert!(!cache_unusable_for(&cache, &dir, &team("team-a")));
    // A different identity is unusable
    assert!(cache_unusable_for(&cache, &dir, &team("team-b")));
    // A deleted artifact is unusable
    std::fs::remove_file(dir.join("requirements.toml")).unwrap();
    assert!(cache_unusable_for(&cache, &dir, &team("team-a")));
    // When armed, unsigned policy is still hard-stale
    std::fs::write(dir.join("requirements.toml"), "[features]\n").unwrap();
    assert!(is_managed_config_hard_stale_for_at(&dir, &team("team-a")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// No marker is hard-stale (never synced, so fetch before use).
#[test]
fn hard_stale_without_marker() {
    let dir = std::env::temp_dir().join(format!("grok-hardstale-nomark-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _ = std::fs::remove_file(dir.join(MANAGED_CONFIG_CACHE_FILE));
    assert!(is_managed_config_hard_stale_for_at(&dir, &team("team-a")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A corrupt marker reads as "no marker": the gate ALLOWS (corruption or a torn write must not lock a managed user out).
/// The cache is hard-stale so the next sync rewrites it.
#[test]
fn corrupt_marker_reads_as_no_marker_and_allows() {
    let dir = std::env::temp_dir().join(format!("grok-corrupt-marker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("requirements.toml"), "fail_closed = true\n").unwrap();
    std::fs::write(dir.join(MANAGED_CONFIG_CACHE_FILE), "{ not valid json").unwrap();

    assert!(read_managed_config_cache(&dir).is_none());
    // No usable marker means not compromised, so corruption can't lock a managed user out...
    assert!(!managed_policy_compromised_for_at(&dir, &team("team-a")));
    // ...but the cache reads hard-stale, so the next sync refetches and rewrites the marker.
    assert!(is_managed_config_hard_stale_for_at(&dir, &team("team-a")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The marker keys on the principal alone and writes no `key_fingerprint`.
#[test]
fn team_path_keys_on_principal_not_key_fingerprint() {
    let dir = std::env::temp_dir().join(format!("grok-team-nofp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: false,
        },
    );
    std::fs::write(dir.join("requirements.toml"), "[features]\n").unwrap();
    let cache = read_managed_config_cache(&dir).unwrap();
    assert!(!cache_unusable_for(&cache, &dir, &team("team-a")));
    // A team switch is detected via principal
    assert!(cache_unusable_for(&cache, &dir, &team("team-b")));
    assert!(is_managed_config_hard_stale_for_at(&dir, &team("team-b")));
    let marker = std::fs::read_to_string(dir.join(MANAGED_CONFIG_CACHE_FILE)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&marker).unwrap();
    assert!(
        v.get("key_fingerprint").is_none(),
        "the marker must not carry a key_fingerprint key: {marker}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A marker from an older binary that still carries `key_fingerprint` reads.
#[test]
fn legacy_marker_with_key_fingerprint_still_reads() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(MANAGED_CONFIG_CACHE_FILE),
        r#"{"synced_at":1,"principal":"team-a","key_fingerprint":"fp-old"}"#,
    )
    .unwrap();
    assert!(read_managed_config_cache(dir.path()).is_some());
    assert_eq!(
        confirmed_team_switch_at(dir.path(), "team-b").as_deref(),
        Some("team-a")
    );
}

/// The eviction trigger fires only on a confirmed switch; first sync, same identity, `None`, and pre-upgrade markers never fire.
#[test]
fn identity_changed_only_on_confirmed_switch() {
    let dir = std::env::temp_dir().join(format!("grok-ident-changed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // No marker yet, so this is the first sync and there is nothing to evict
    assert!(!managed_config_identity_changed_at(&dir, Some("team-a")));

    // Team marker: the same team is no switch, a different team is a switch, and unknown (None) never evicts
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: true,
            had_requirements: true,
            fail_closed: false,
        },
    );
    assert!(!managed_config_identity_changed_at(&dir, Some("team-a")));
    assert!(managed_config_identity_changed_at(&dir, Some("team-b")));
    assert!(!managed_config_identity_changed_at(&dir, None));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A blank/whitespace principal is "unknown", never a distinct identity, on EITHER side.
/// A malformed `auth.json` or corrupt marker must not make the gate purge / apply eviction shed a real tenant's policy.
#[test]
fn blank_principal_is_never_a_confirmed_switch() {
    let dir = std::env::temp_dir().join(format!("grok-ident-blank-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // A real recorded team with a blank current principal is not a switch
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: true,
            had_requirements: true,
            fail_closed: false,
        },
    );
    for blank in ["", "   "] {
        assert!(
            !managed_config_identity_changed_at(&dir, Some(blank)),
            "a blank current principal ({blank:?}) must not read as a confirmed switch"
        );
    }

    // A blank recorded principal with a real current one is not a switch either
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("  "),
            had_managed_config: true,
            had_requirements: true,
            fail_closed: false,
        },
    );
    assert!(
        !managed_config_identity_changed_at(&dir, Some("team-b")),
        "a blank recorded principal must not read as a distinct identity"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Armed: fail-closed with served policy requires an authentic sidecar.
#[test]
fn compromised_only_when_opted_in_and_deleted_or_substituted() {
    let dir = std::env::temp_dir().join(format!("grok-compromised-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // No marker means not compromised
    let _ = std::fs::remove_file(dir.join(MANAGED_CONFIG_CACHE_FILE));
    assert!(!managed_policy_compromised_for_at(&dir, &team("team-a")));

    // Opted-in with no sidecar refuses when armed
    std::fs::write(dir.join("requirements.toml"), "[features]\n").unwrap();
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: true,
        },
    );
    assert!(managed_policy_compromised_for_at(&dir, &team("team-a")));

    // Served-then-deleted (admin opted in) is compromised
    std::fs::remove_file(dir.join("requirements.toml")).unwrap();
    assert!(managed_policy_compromised_for_at(&dir, &team("team-a")));

    // A different principal with the artifact still missing is compromised by the artifact, not the identity
    assert!(managed_policy_compromised_for_at(&dir, &team("team-b")));

    // When not opted in, a deletion is NOT failed closed
    std::fs::write(dir.join("requirements.toml"), "[features]\n").unwrap();
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: false,
        },
    );
    std::fs::remove_file(dir.join("requirements.toml")).unwrap();
    assert!(!managed_policy_compromised_for_at(&dir, &team("team-a")));

    // A config-less principal (nothing served) is never compromised
    mark_managed_config_synced_at(
        &dir,
        SyncMarker {
            principal: Some("team-c"),
            had_managed_config: false,
            had_requirements: false,
            fail_closed: false,
        },
    );
    assert!(!managed_policy_compromised_for_at(&dir, &team("team-c")));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compromised_on_managed_config_deletion_when_fail_closed() {
    use crate::signed_policy::SignedVerdict;
    let dir = std::env::temp_dir().join(format!("grok-compromised-mc-{}", std::process::id()));
    let home = dir.as_path();
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(home.join("managed_config.toml"), "[cli]\n").unwrap();
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: true,
            had_requirements: false,
            fail_closed: true,
        },
    );
    let cache = read_managed_config_cache(home);
    // With the file present, the marker decision is not compromised
    assert!(!managed_policy_compromised_decision(
        SignedVerdict::Inactive,
        || false,
        cache.as_ref(),
        home,
        &team("team-a")
    ));
    // Served-then-deleted managed_config.toml is compromised by the missing artifact
    std::fs::remove_file(home.join("managed_config.toml")).unwrap();
    assert!(managed_policy_compromised_decision(
        SignedVerdict::Inactive,
        || false,
        cache.as_ref(),
        home,
        &team("team-a")
    ));
    // The armed public gate refuses fail-closed with no sidecar
    assert!(managed_policy_compromised_for_at(home, &team("team-a")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Marker refuses only current-principal tamper, not pure identity mismatch.
#[test]
fn gate_excludes_pure_identity_mismatch_but_keeps_artifact_tamper() {
    use crate::signed_policy::SignedVerdict;
    let dir = std::env::temp_dir().join(format!("grok-gate-fix1-{}", std::process::id()));
    let home = dir.as_path();
    std::fs::create_dir_all(home).unwrap();

    // (1) Principal A (fail_closed), artifact intact; serving team-b is a pure identity mismatch, so it is ALLOWED
    std::fs::write(home.join("requirements.toml"), "[features]\n").unwrap();
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: true,
        },
    );
    let cache = read_managed_config_cache(home);
    assert!(
        !managed_policy_compromised_decision(
            SignedVerdict::Inactive,
            || false,
            cache.as_ref(),
            home,
            &team("team-b")
        ),
        "a foreign/stale principal's fail_closed must NOT refuse on the marker path"
    );
    // ...but still stale for B, so the refetch path rebinds online
    assert!(
        is_managed_config_hard_stale_for_at(home, &team("team-b")),
        "a pure identity mismatch must still trigger a refetch (rebind)"
    );

    // (2) The same principal with the artifact missing refuses offline
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-b"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: true,
        },
    );
    std::fs::remove_file(home.join("requirements.toml")).unwrap();
    let cache = read_managed_config_cache(home);
    assert!(
        managed_policy_compromised_decision(
            SignedVerdict::Inactive,
            || false,
            cache.as_ref(),
            home,
            &team("team-b")
        ),
        "same-principal served-then-deleted artifact must fail closed offline"
    );
    assert!(managed_policy_compromised_for_at(home, &team("team-b")));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Opt-in comes from the response, not disk; a no-write sync cannot disarm.
#[test]
fn mark_keeps_fail_closed_armed_without_on_disk_file() {
    use crate::signed_policy::SignedVerdict;
    let dir = std::env::temp_dir().join(format!("grok-mark-disarm-{}", std::process::id()));
    let home = dir.as_path();
    std::fs::create_dir_all(home).unwrap();

    // Opted-in with the file present allows (marker)
    std::fs::write(home.join("requirements.toml"), "[features]\n").unwrap();
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-1"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: true,
        },
    );
    let cache = read_managed_config_cache(home);
    assert!(!managed_policy_compromised_decision(
        SignedVerdict::Inactive,
        || false,
        cache.as_ref(),
        home,
        &team("team-1")
    ));

    // Once the served file is deleted, the decision is compromised
    std::fs::remove_file(home.join("requirements.toml")).unwrap();
    assert!(managed_policy_compromised_decision(
        SignedVerdict::Inactive,
        || false,
        cache.as_ref(),
        home,
        &team("team-1")
    ));
    assert!(managed_policy_compromised_for_at(home, &team("team-1")));

    // A no-write sync (file still absent) stays armed: opt-in is from the response.
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-1"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: true,
        },
    );
    assert!(
        managed_policy_compromised_for_at(home, &team("team-1")),
        "a no-write sync must not disarm the fail-closed gate"
    );

    // fail_closed=false still takes effect
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-1"),
            had_managed_config: false,
            had_requirements: true,
            fail_closed: false,
        },
    );
    assert!(!managed_policy_compromised_for_at(home, &team("team-1")));

    let _ = std::fs::remove_dir_all(&dir);
}

/// The offline purge detector fires only on a marker-recorded TEAM switch, returning the evicted principal.
#[test]
fn confirmed_team_switch_scopes_to_marker() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();

    // No marker, no switch (first run / signed-out)
    assert_eq!(confirmed_team_switch_at(home, "team-b"), None);

    // A team marker switch from A to B confirms and reports the evicted principal; the same team doesn't
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: true,
            had_requirements: true,
            fail_closed: true,
        },
    );
    assert_eq!(
        confirmed_team_switch_at(home, "team-b").as_deref(),
        Some("team-a")
    );
    assert_eq!(confirmed_team_switch_at(home, "team-a"), None);
}

/// Blank identity values normalize to `None` at the marker WRITE, so no reader can treat "unknown" as a distinct tenant.
/// The detectors' blank guards stay as defense in depth.
#[test]
fn marker_write_normalizes_blank_identities() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("   "),
            had_managed_config: true,
            had_requirements: true,
            fail_closed: true,
        },
    );
    let cache = read_managed_config_cache(home).expect("marker written");
    assert_eq!(
        cache.principal, None,
        "blank principal must not be recorded"
    );
    // And a blank-recorded marker can't confirm a switch.
    assert_eq!(confirmed_team_switch_at(home, "team-b"), None);
}

/// Identity values are stored TRIMMED at the marker write, so a marker can never differ from a live value by surrounding whitespace alone.
#[test]
fn marker_write_trims_identity_values() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("  team-a  "),
            had_managed_config: true,
            had_requirements: true,
            fail_closed: false,
        },
    );
    let cache = read_managed_config_cache(home).expect("marker written");
    assert_eq!(cache.principal.as_deref(), Some("team-a"));
}

/// The blank and trim rules live only here ([`known`] and `confirmed_switch`).
/// A switch needs both sides known and differing on their trimmed forms, else `None`.
#[test]
fn confirmed_switch_requires_two_known_differing_sides() {
    assert_eq!(confirmed_switch(Some("a"), Some("b")), Some("a"));
    assert_eq!(confirmed_switch(Some("a"), Some("a")), None);
    assert_eq!(confirmed_switch(Some(" "), Some("b")), None);
    assert_eq!(confirmed_switch(Some("a"), Some("")), None);
    assert_eq!(confirmed_switch(None, Some("b")), None);
    assert_eq!(confirmed_switch(Some("a"), None), None);
    assert_eq!(confirmed_switch(None, None), None);
    // Whitespace is not identity: a marker written untrimmed by an older build must not read as a tenant switch against the same (trimmed) value...
    assert_eq!(confirmed_switch(Some("team-a "), Some("team-a")), None);
    assert_eq!(confirmed_switch(Some("team-a"), Some("team-a ")), None);
    // ...while genuinely different trimmed values still switch (the recorded value is returned verbatim for logging)
    assert_eq!(
        confirmed_switch(Some(" team-a "), Some("team-b")),
        Some(" team-a ")
    );
}

/// The staleness identity compare is trim-aware (a sibling of the trim at the marker write).
#[test]
fn cache_identity_mismatch_ignores_whitespace_only_diffs() {
    let cache = ManagedConfigCache {
        principal: Some("team-a".into()),
        ..Default::default()
    };
    assert!(
        !cache_identity_mismatch(&cache, &team(" team-a ")),
        "whitespace-only team id diff must not hard-stale"
    );
    assert!(
        cache_identity_mismatch(&cache, &team("team-b")),
        "a real team switch must still mismatch"
    );
    // One-sided known still mismatches (first install / cleared marker fields).
    let empty = ManagedConfigCache::default();
    assert!(cache_identity_mismatch(&empty, &team("team-a")));
}

/// Tick raises an existing floor, never lowers it, and never creates a marker.
#[test]
fn rollback_floor_ticks_up_never_down_and_never_creates_a_marker() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let floor = |home: &Path| read_managed_config_cache(home).map_or(0, |c| c.rollback_floor);

    raise_rollback_floor(home, 5_000);
    assert!(
        read_managed_config_cache(home).is_none(),
        "the tick must not create a marker"
    );

    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: false,
            fail_closed: false,
        },
    );
    let base = floor(home);
    assert!(
        base >= 1_700_000_000,
        "a fetch seeds the floor at the wall clock"
    );

    raise_rollback_floor(home, base + 1_000);
    assert_eq!(floor(home), base + 1_000);
    raise_rollback_floor(home, base);
    assert_eq!(floor(home), base + 1_000, "the tick never lowers the floor");
}

/// The floor RMW preserves marker fields this binary doesn't know (mixed-version homes).
#[test]
fn floor_bump_preserves_unknown_marker_fields() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(
        home.join(MANAGED_CONFIG_CACHE_FILE),
        r#"{"synced_at":1700000000,"rollback_floor":1700000000,"from_the_future":true}"#,
    )
    .unwrap();
    raise_rollback_floor(home, 1_700_000_100);
    let marker = std::fs::read_to_string(home.join(MANAGED_CONFIG_CACHE_FILE)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&marker).unwrap();
    assert_eq!(
        v.get("rollback_floor").and_then(serde_json::Value::as_u64),
        Some(1_700_000_100)
    );
    assert_eq!(
        v.get("from_the_future"),
        Some(&serde_json::Value::Bool(true)),
        "the RMW must not strip fields a newer binary wrote: {marker}"
    );
}

/// Successful fetch resets (never maxes) an inflated floor to the wall clock.
#[test]
fn fetch_resets_an_inflated_rollback_floor() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::write(
        home.join(MANAGED_CONFIG_CACHE_FILE),
        r#"{"rollback_floor":9999999999}"#,
    )
    .unwrap();

    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: false,
            fail_closed: false,
        },
    );
    let floor = read_managed_config_cache(home).map_or(0, |c| c.rollback_floor);
    assert!(
        (1_700_000_000..9_999_999_999).contains(&floor),
        "the fetch must reset the inflated floor to the wall clock, got {floor}"
    );
}

#[test]
fn bump_rollback_floor_is_inert_when_dark() {
    crate::signed_policy::test_seam::with_dark(|| {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        mark_managed_config_synced_at(
            home,
            SyncMarker {
                principal: Some("team-a"),
                had_managed_config: false,
                had_requirements: false,
                fail_closed: false,
            },
        );
        let floor = |home: &Path| read_managed_config_cache(home).map_or(0, |c| c.rollback_floor);
        let base = floor(home);
        assert!(!crate::signed_policy::verification_active());
        bump_rollback_floor_with_now(home, base + 10_000);
        assert_eq!(
            floor(home),
            base,
            "dark build: the tick must not move the floor"
        );
    });
}

#[test]
fn bump_rollback_floor_raises_when_verification_active() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    mark_managed_config_synced_at(
        home,
        SyncMarker {
            principal: Some("team-a"),
            had_managed_config: false,
            had_requirements: false,
            fail_closed: false,
        },
    );
    let floor = |home: &Path| read_managed_config_cache(home).map_or(0, |c| c.rollback_floor);
    let base = floor(home);
    assert!(crate::signed_policy::verification_active());
    let raised = base + 10_000;
    bump_rollback_floor_with_now(home, raised);
    assert_eq!(
        floor(home),
        raised,
        "armed build: the tick must raise the floor"
    );
}

/// Far-future `synced_at` is stale; modest forward skew stays fresh.
#[test]
fn managed_config_stale_for_far_future_sync() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // ~year 3000: beyond the skew allowance.
    std::fs::write(
        home.join(MANAGED_CONFIG_CACHE_FILE),
        "{\"synced_at\":32503680000}",
    )
    .unwrap();
    assert!(
        managed_config_stale_at(Some(home), &ServingIdentity::None),
        "a far-future synced_at must not freeze the refetch timer"
    );

    // Past `SystemTime`'s range: must read stale, not panic (would kill the sync task).
    std::fs::write(
        home.join(MANAGED_CONFIG_CACHE_FILE),
        format!("{{\"synced_at\":{}}}", u64::MAX),
    )
    .unwrap();
    assert!(
        managed_config_stale_at(Some(home), &ServingIdentity::None),
        "an out-of-range synced_at reads stale"
    );

    let in_a_minute = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 60;
    std::fs::write(
        home.join(MANAGED_CONFIG_CACHE_FILE),
        format!("{{\"synced_at\":{in_a_minute}}}"),
    )
    .unwrap();
    assert!(
        !managed_config_stale_at(Some(home), &ServingIdentity::None),
        "a minute of genuine clock skew still reads fresh"
    );
}

/// Unreadable requirements (PermissionDenied) with no fail_closed marker must still arm the gate.
/// Otherwise clear_orphan could wipe policy that may still be fail_closed on disk.
#[test]
#[cfg(unix)]
fn unreadable_requirements_treats_fail_closed_as_armed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let req = home.join(crate::loader::REQUIREMENTS_FILENAME);
    std::fs::write(&req, "fail_closed = true\n").unwrap();
    assert!(
        fail_closed_policy_armed_at(home),
        "readable fail_closed requirements must arm the gate"
    );

    // Drop read perms so read_to_string fails with PermissionDenied (not NotFound).
    std::fs::set_permissions(&req, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Restore on drop so tempfile cleanup can remove the file.
    struct RestorePerms<'a>(&'a std::path::Path);
    impl Drop for RestorePerms<'_> {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o600));
        }
    }
    let _restore = RestorePerms(&req);

    assert!(
        fail_closed_policy_armed_at(home),
        "unreadable requirements must treat fail_closed as armed (no wipe)"
    );
}

/// Absent requirements and no fail_closed marker do not arm the gate (safe to clear).
#[test]
fn missing_requirements_and_marker_not_armed() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        !fail_closed_policy_armed_at(dir.path()),
        "NotFound requirements with no marker must not arm fail_closed"
    );
}

// The is-managed claim gate tests live in a sibling child module (this file is past the 1k-line mark)
// The #[path] include below keeps the same private access
#[path = "claim_tests.rs"]
mod claim_tests;
