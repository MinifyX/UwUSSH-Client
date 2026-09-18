//! Two devices, one server, no network.
//!
//! Every test here runs the real store and the real crypto against
//! [`MemoryServer`], which follows the server's rules. What is checked is what
//! sync has to get right: a change reaches the other device, a conflict ends
//! the same way on both, a delete stays deleted — and a server that tampers
//! with what it stores gets nowhere.

use uuid::Uuid;
use uwussh_proto::{EntityKind, Envelope, Extra, Hlc, HostPayload, Resolution, Version, MAX_BATCH};
use uwussh_store::{AuthMethod, HostDraft, PasswordChange, SecretText, Store, Workspace};
use uwussh_vault::KdfParams;

use crate::{sync_once, MemoryServer, SyncError};

const PASSWORD: &[u8] = b"correct horse battery staple";

/// The first device: a store with a vault of its own.
fn first_device() -> Store {
    let store = Store::open_in_memory().unwrap();
    store
        .create_vault_with(PASSWORD, KdfParams::INSECURE_FOR_TESTS)
        .unwrap();
    store
}

/// A second device joining that vault: it takes the header the server holds
/// and opens it with the master password, exactly as pairing will.
fn joined_device(first: &Store) -> Store {
    let header = first.vault_header().unwrap().expect("a vault to join");
    let key = header.unlock(PASSWORD).unwrap().export_key();
    let store = Store::open_in_memory().unwrap();
    store.adopt_vault(&header, key).unwrap();
    store
}

fn draft(name: &str, address: &str) -> HostDraft {
    HostDraft {
        id: None,
        name: name.into(),
        address: address.into(),
        port: 22,
        username: "uwu".into(),
        auth: AuthMethod::Password,
        key_path: None,
        group_path: None,
        workspace: None,
        key_id: None,
        password: PasswordChange::Keep,
    }
}

fn names(store: &Store) -> Vec<String> {
    store
        .list_hosts()
        .unwrap()
        .into_iter()
        .map(|host| host.name)
        .collect()
}

#[test]
fn what_one_device_adds_shows_up_on_the_other() {
    let server = MemoryServer::new();
    let a = first_device();
    let b = joined_device(&a);

    let mut host = draft("prox-1", "10.0.0.12");
    host.group_path = Some("Homelab".into());
    host.password = PasswordChange::Set {
        value: SecretText::new("hunter2"),
    };
    let saved = a.save_host(host).unwrap();

    let up = sync_once(&a, &server).unwrap();
    assert!(up.pushed >= 4, "host, login, group and password: {up:?}");
    assert_eq!(
        up.apply.identical, up.pushed,
        "the pass ends by reading back what it wrote"
    );
    assert_eq!(up.conflicts, 0);

    let down = sync_once(&b, &server).unwrap();
    assert_eq!(down.pulled, up.pushed);
    assert_eq!(down.apply.rejected, 0);
    assert_eq!(down.apply.applied, up.pushed, "all of it was new to B");

    let hosts = b.list_hosts().unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].id, saved.id, "the id travels with the record");
    assert_eq!(hosts[0].address, "10.0.0.12");
    assert_eq!(hosts[0].username, "uwu");
    assert_eq!(
        hosts[0].group_path.as_deref(),
        Some("Homelab"),
        "the group came along as a record of its own"
    );
    assert!(hosts[0].has_password);
    assert_eq!(
        b.reveal_host_password(saved.id).unwrap().as_slice(),
        b"hunter2",
        "the password opens on the other device"
    );
    assert_eq!(b.list_groups().unwrap().len(), 1);
}

#[test]
fn a_host_i_removed_stays_removed_on_the_other_device() {
    let server = MemoryServer::new();
    let a = first_device();
    let b = joined_device(&a);
    let host = a.save_host(draft("nas", "10.0.0.9")).unwrap();
    sync_once(&a, &server).unwrap();
    sync_once(&b, &server).unwrap();
    assert_eq!(names(&b), vec!["nas"]);

    a.delete_host(host.id).unwrap();
    sync_once(&a, &server).unwrap();
    let report = sync_once(&b, &server).unwrap();
    assert!(report.apply.applied >= 1);
    assert!(names(&b).is_empty(), "the tombstone travelled");

    // And a device that edited it while offline does not bring it back.
    let c = joined_device(&a);
    sync_once(&c, &server).unwrap();
    assert!(names(&c).is_empty());
}

#[test]
fn the_later_of_two_edits_wins_on_both_devices() {
    let server = MemoryServer::new();
    let a = first_device();
    let b = joined_device(&a);
    let host = a.save_host(draft("web", "10.0.0.20")).unwrap();
    sync_once(&a, &server).unwrap();
    sync_once(&b, &server).unwrap();

    // Both rename it while neither has synced. A moment in between, so B's
    // edit really is the later one: within one millisecond the clocks tie and
    // the winner comes down to which device id sorts higher, which is a coin
    // flip and not what this test is about.
    let mut on_a = draft("web-a", "10.0.0.20");
    on_a.id = Some(host.id);
    a.save_host(on_a).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(3));
    let mut on_b = draft("web-b", "10.0.0.20");
    on_b.id = Some(host.id);
    b.save_host(on_b).unwrap();

    // A gets there first, B's push is refused and merged.
    sync_once(&a, &server).unwrap();
    let report = sync_once(&b, &server).unwrap();
    assert!(report.conflicts >= 1, "the server refused B's version");
    assert!(report.rounds <= crate::MAX_ROUNDS);

    // Whoever won, both devices agree — and so does the server.
    sync_once(&a, &server).unwrap();
    sync_once(&b, &server).unwrap();
    assert_eq!(names(&a), names(&b), "the two devices agree");
    assert_eq!(names(&b), vec!["web-b"], "B wrote later, so B wins");
}

#[test]
fn syncing_again_finds_nothing_to_do() {
    let server = MemoryServer::new();
    let a = first_device();
    a.save_host(draft("one", "10.0.0.1")).unwrap();
    sync_once(&a, &server).unwrap();

    let again = sync_once(&a, &server).unwrap();
    assert_eq!(again.pushed, 0);
    assert_eq!(
        again.pulled, 0,
        "pushing last would have left work for the next pass"
    );
    assert_eq!(again.rounds, 1);
    assert_eq!(a.pending_count().unwrap(), 0);

    let b = joined_device(&a);
    sync_once(&b, &server).unwrap();
    let again = sync_once(&b, &server).unwrap();
    assert_eq!((again.pulled, again.pushed), (0, 0));
}

#[test]
fn the_server_holds_nothing_it_can_read() {
    let server = MemoryServer::new();
    let a = first_device();
    let mut host = draft("prox-1", "10.0.0.12");
    host.group_path = Some("Homelab".into());
    host.password = PasswordChange::Set {
        value: SecretText::new("hunter2"),
    };
    a.save_host(host).unwrap();
    a.trust_host_key(
        "10.0.0.12",
        22,
        "ssh-ed25519",
        "SHA256:abcdef",
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5",
    )
    .unwrap();
    sync_once(&a, &server).unwrap();

    let held = server.records();
    assert!(!held.is_empty());
    for env in &held {
        let haystack = String::from_utf8_lossy(&env.blob).to_string();
        for secret in ["prox-1", "10.0.0.12", "hunter2", "Homelab", "SHA256:abcdef"] {
            assert!(
                !haystack.contains(secret),
                "{secret} must not be readable in a blob"
            );
        }
        assert_eq!(env.nonce.len(), 24);
    }
    // What it does see: how many records there are, of what kind, and when.
    assert!(held.iter().any(|env| env.kind == EntityKind::Host));
}

#[test]
fn a_server_that_marks_a_record_deleted_gets_nowhere() {
    let server = MemoryServer::new();
    let a = first_device();
    let b = joined_device(&a);
    a.save_host(draft("nas", "10.0.0.9")).unwrap();
    sync_once(&a, &server).unwrap();
    sync_once(&b, &server).unwrap();
    assert_eq!(names(&b), vec!["nas"]);

    // The server flips the tombstone flag on the host — the one bit that
    // would otherwise remove it everywhere, since a delete beats an edit.
    let mut host = server
        .records()
        .into_iter()
        .find(|env| env.kind == EntityKind::Host)
        .unwrap();
    host.deleted = true;
    host.updated_at = Hlc::new(host.updated_at.wall_ms + 60_000, 0, host.updated_at.device);
    server.store(host);

    let report = sync_once(&b, &server).unwrap();
    assert_eq!(report.apply.rejected, 1, "the seal caught it");
    assert_eq!(report.apply.applied, 0);
    assert_eq!(names(&b), vec!["nas"], "the host is still there");
}

#[test]
fn a_server_that_replays_an_old_version_gets_nowhere() {
    let server = MemoryServer::new();
    let a = first_device();
    let b = joined_device(&a);
    let host = a.save_host(draft("bastion", "10.0.0.2")).unwrap();
    sync_once(&a, &server).unwrap();
    let old = server
        .records()
        .into_iter()
        .find(|env| env.kind == EntityKind::Host)
        .unwrap();

    let mut renamed = draft("bastion-new", "10.0.0.2");
    renamed.id = Some(host.id);
    a.save_host(renamed).unwrap();
    sync_once(&a, &server).unwrap();
    sync_once(&b, &server).unwrap();
    assert_eq!(names(&b), vec!["bastion-new"]);

    // The server hands the old blob back under a newer clock.
    let mut replay = old.clone();
    replay.updated_at = Hlc::new(
        replay.updated_at.wall_ms + 3_600_000,
        0,
        replay.updated_at.device,
    );
    server.store(replay);

    let report = sync_once(&b, &server).unwrap();
    assert_eq!(report.apply.rejected, 1);
    assert_eq!(
        names(&b),
        vec!["bastion-new"],
        "the old name did not come back"
    );

    // The same blob with its own clock is simply older, and loses.
    server.store(old);
    let report = sync_once(&b, &server).unwrap();
    assert_eq!(report.apply.rejected, 0);
    assert_eq!(report.apply.kept, 1);
    assert_eq!(names(&b), vec!["bastion-new"]);
}

#[test]
fn a_record_from_another_vault_is_dropped() {
    let server = MemoryServer::new();
    let a = first_device();
    a.save_host(draft("one", "10.0.0.1")).unwrap();
    sync_once(&a, &server).unwrap();

    let mut stranger = server
        .records()
        .into_iter()
        .find(|env| env.kind == EntityKind::Host)
        .unwrap();
    stranger.id = Uuid::now_v7();
    stranger.vault_id = Uuid::now_v7();
    server.store(stranger);

    let report = sync_once(&a, &server).unwrap();
    assert_eq!(report.apply.rejected, 1);
    assert_eq!(names(&a), vec!["one"]);
}

#[test]
fn two_devices_that_trusted_different_keys_for_one_host_end_up_agreeing() {
    let server = MemoryServer::new();
    let a = first_device();
    let b = joined_device(&a);

    a.trust_host_key(
        "nas.lan",
        22,
        "ssh-ed25519",
        "SHA256:aaa",
        "ssh-ed25519 AAAA",
    )
    .unwrap();
    // A moment later, or the two decisions share a millisecond and the tie
    // falls to whichever device id is greater — which is not what is under
    // test here.
    std::thread::sleep(std::time::Duration::from_millis(2));
    b.trust_host_key(
        "nas.lan",
        22,
        "ssh-ed25519",
        "SHA256:bbb",
        "ssh-ed25519 BBBB",
    )
    .unwrap();

    sync_once(&a, &server).unwrap();
    let report = sync_once(&b, &server).unwrap();
    assert_eq!(
        report.apply.host_key_conflicts, 1,
        "it is worth telling the user about"
    );
    sync_once(&a, &server).unwrap();
    sync_once(&b, &server).unwrap();

    let on_a = a.known_host("nas.lan", 22).unwrap().unwrap();
    let on_b = b.known_host("nas.lan", 22).unwrap().unwrap();
    assert_eq!(
        on_a.fingerprint, on_b.fingerprint,
        "one key per address, the same one on both devices"
    );
    assert_eq!(
        on_b.fingerprint, "SHA256:bbb",
        "the later decision takes the slot"
    );
}

#[test]
fn a_field_a_newer_build_wrote_is_not_dropped_by_this_one() {
    let server = MemoryServer::new();
    let a = first_device();
    let b = joined_device(&a);

    // A build from the future writes a host with a field this one has never
    // heard of, sealed with the same vault key.
    let header = a.vault_header().unwrap().unwrap();
    let vault = header.unlock(PASSWORD).unwrap();
    let id = Uuid::now_v7();
    let clock = Hlc::new(1_700_000_000_000, 0, 42);
    let mut extra = Extra::new();
    extra.insert("tags".into(), serde_json::json!(["homelab", "critical"]));
    let payload = serde_json::to_vec(&HostPayload {
        name: "future".into(),
        address: "10.0.0.77".into(),
        port: 22,
        workspace: "private".into(),
        position: 0,
        group_id: None,
        identity_id: None,
        extra,
    })
    .unwrap();
    let sealed = vault
        .seal_synced(id, EntityKind::Host, clock, false, &payload)
        .unwrap();
    server.store(Envelope {
        id,
        vault_id: vault.vault_id(),
        kind: EntityKind::Host,
        updated_at: clock,
        base_seq: 0,
        deleted: false,
        nonce: sealed.nonce,
        blob: sealed.blob,
        seq: None,
    });

    sync_once(&a, &server).unwrap();
    assert_eq!(names(&a), vec!["future"]);

    // This build edits the host and pushes it back.
    let mut edit = draft("future-renamed", "10.0.0.77");
    edit.id = Some(id);
    a.save_host(edit).unwrap();
    sync_once(&a, &server).unwrap();
    sync_once(&b, &server).unwrap();

    // The newer build's field is still in what the server holds.
    let host = server
        .records()
        .into_iter()
        .find(|env| env.id == id)
        .unwrap();
    let opened = vault
        .open_synced(
            host.id,
            host.kind,
            host.updated_at,
            host.deleted,
            &uwussh_vault::Sealed {
                nonce: host.nonce.clone(),
                blob: host.blob.clone(),
            },
        )
        .unwrap();
    let payload: HostPayload = serde_json::from_slice(&opened).unwrap();
    assert_eq!(payload.name, "future-renamed");
    assert_eq!(
        payload.extra.get("tags"),
        Some(&serde_json::json!(["homelab", "critical"])),
        "the tags survived an edit by a build that knows nothing about them"
    );
}

#[test]
fn a_host_whose_login_has_not_arrived_yet_is_still_shown() {
    let server = MemoryServer::new();
    let a = first_device();
    let b = joined_device(&a);
    let mut host = draft("half", "10.0.0.5");
    host.group_path = Some("Later".into());
    a.save_host(host).unwrap();
    sync_once(&a, &server).unwrap();

    // Only the host itself arrives — its login and its group are in a page
    // that has not come yet.
    let held = server.records();
    let only_host: Vec<_> = held
        .iter()
        .filter(|env| env.kind == EntityKind::Host)
        .cloned()
        .collect();
    let report = b.apply_envelopes(&only_host).unwrap();
    assert_eq!(report.applied, 1);
    let hosts = b.list_hosts().unwrap();
    assert_eq!(hosts.len(), 1, "the host is there, with placeholders");
    assert_eq!(hosts[0].username, "");
    assert_eq!(
        hosts[0].group_path, None,
        "a group without a name yet is not shown as one"
    );
    assert!(
        b.list_groups().unwrap().is_empty(),
        "and it is not in the group list either"
    );
    assert_eq!(
        b.pending_count().unwrap(),
        0,
        "a placeholder is never pushed"
    );

    // The rest arrives.
    let report = sync_once(&b, &server).unwrap();
    assert_eq!(report.apply.rejected, 0);
    let hosts = b.list_hosts().unwrap();
    assert_eq!(hosts[0].username, "uwu");
    assert_eq!(hosts[0].group_path.as_deref(), Some("Later"));
    assert_eq!(b.pending_count().unwrap(), 0);
}

#[test]
fn a_device_that_joins_brings_its_own_hosts_along() {
    let server = MemoryServer::new();
    let a = first_device();
    a.save_host(draft("from-a", "10.0.0.1")).unwrap();
    sync_once(&a, &server).unwrap();

    // B had a vault and a host of its own before it ever heard of a server.
    let b = Store::open_in_memory().unwrap();
    b.create_vault_with(b"b's own password", KdfParams::INSECURE_FOR_TESTS)
        .unwrap();
    let mut own = draft("from-b", "10.0.0.2");
    own.password = PasswordChange::Set {
        value: SecretText::new("b-secret"),
    };
    let own = b.save_host(own).unwrap();

    let header = a.vault_header().unwrap().unwrap();
    let key = header.unlock(PASSWORD).unwrap().export_key();
    b.adopt_vault(&header, key).unwrap();
    assert_eq!(
        b.reveal_host_password(own.id).unwrap().as_slice(),
        b"b-secret",
        "its own secrets were sealed again under the account's key"
    );

    sync_once(&b, &server).unwrap();
    sync_once(&a, &server).unwrap();

    let mut on_a = names(&a);
    on_a.sort();
    assert_eq!(on_a, vec!["from-a", "from-b"]);
    assert_eq!(
        a.reveal_host_password(own.id).unwrap().as_slice(),
        b"b-secret",
        "and they open on the first device too"
    );
}

#[test]
fn a_pass_without_a_vault_asks_for_the_password_instead_of_failing() {
    let server = MemoryServer::new();
    let a = first_device();
    a.save_host(draft("one", "10.0.0.1")).unwrap();
    a.lock_vault();
    assert!(matches!(
        sync_once(&a, &server),
        Err(SyncError::VaultLocked)
    ));
    assert!(server.is_empty(), "and nothing left the device");
}

#[test]
fn a_batch_larger_than_the_protocol_allows_is_refused_by_the_server() {
    use crate::Transport;
    let server = MemoryServer::new();
    let a = first_device();
    let envelopes = vec![
        Envelope {
            id: Uuid::now_v7(),
            vault_id: a.vault_header().unwrap().unwrap().vault_id,
            kind: EntityKind::Host,
            updated_at: Hlc::new(1, 0, 1),
            base_seq: 0,
            deleted: false,
            nonce: vec![0; 24],
            blob: vec![0; 32],
            seq: None,
        };
        MAX_BATCH + 1
    ];
    assert!(server.push(envelopes).is_err());
}

#[test]
fn the_conflict_rule_is_the_one_both_sides_share() {
    // The rule lives in the protocol crate, so the server and both devices
    // cannot drift apart on it. Spot-check that this is the rule in force.
    let older = Version::new(Hlc::new(100, 0, 1), false);
    let newer = Version::new(Hlc::new(200, 0, 2), false);
    assert_eq!(uwussh_proto::resolve(newer, older), Resolution::Local);
    assert_eq!(uwussh_proto::resolve(older, newer), Resolution::Remote);
    let tombstone = Version::new(Hlc::new(1, 0, 1), true);
    assert_eq!(uwussh_proto::resolve(newer, tombstone), Resolution::Remote);
}

#[test]
fn a_workspace_this_build_does_not_know_lands_in_the_private_one() {
    let server = MemoryServer::new();
    let a = first_device();
    let header = a.vault_header().unwrap().unwrap();
    let vault = header.unlock(PASSWORD).unwrap();
    let id = Uuid::now_v7();
    let clock = Hlc::new(1_700_000_000_000, 0, 9);
    let payload = serde_json::to_vec(&HostPayload {
        name: "third".into(),
        address: "10.0.0.33".into(),
        port: 22,
        workspace: "family".into(),
        position: 0,
        group_id: None,
        identity_id: None,
        extra: Extra::new(),
    })
    .unwrap();
    let sealed = vault
        .seal_synced(id, EntityKind::Host, clock, false, &payload)
        .unwrap();
    server.store(Envelope {
        id,
        vault_id: vault.vault_id(),
        kind: EntityKind::Host,
        updated_at: clock,
        base_seq: 0,
        deleted: false,
        nonce: sealed.nonce,
        blob: sealed.blob,
        seq: None,
    });

    let report = sync_once(&a, &server).unwrap();
    assert_eq!(report.apply.applied, 1);
    let hosts = a.list_hosts().unwrap();
    assert_eq!(hosts[0].workspace, Workspace::Private);
}
