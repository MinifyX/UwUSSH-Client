//! The Termius import against a Termius database built by hand, with the
//! layout found in Termius 10: one IndexedDB database per entity, sealed
//! fields, `content` copies, references by `local_id`.

use super::*;
use crate::chromium::idb::testing::{database_name, record, store_name};
use crate::chromium::leveldb::testing::{batch, log, Op};
use crate::chromium::v8::testing::Writer;
use seal::testing::seal;

const KEY: [u8; 32] = [42; 32];
const TEAM_KEY: [u8; 32] = [43; 32];

enum F<'a> {
    Sealed(&'a str),
    SealedWith([u8; 32], &'a str),
    Plain(&'a str),
    Num(f64),
    Bool(bool),
    Null,
    Ref(i64),
}

use F::*;

/// A Termius record: `id`, `local_id` and `status`, then the given fields.
fn entity(local_id: i64, status: &str, fields: &[(&str, F)]) -> Vec<u8> {
    let mut w = Writer::new();
    w.begin_object()
        .string("id")
        .double(local_id as f64 + 1000.0)
        .string("local_id")
        .double(local_id as f64)
        .string("status")
        .string(status);
    for (name, value) in fields {
        w.string(name);
        match value {
            Sealed(text) => w.string(&seal(&KEY, [9; 24], text.as_bytes())),
            SealedWith(key, text) => w.string(&seal(key, [9; 24], text.as_bytes())),
            Plain(text) => w.string(text),
            Num(n) => w.double(*n),
            Bool(b) => w.bool(*b),
            Null => w.null(),
            Ref(id) => w
                .begin_object()
                .string("id")
                .double(*id as f64 + 1000.0)
                .string("local_id")
                .double(*id as f64)
                .end_object(2),
        };
    }
    w.end_object(fields.len() as u64 + 3);
    w.bytes()
}

struct Fixture {
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    next_database: u8,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            entries: Vec::new(),
            next_database: 1,
        }
    }

    fn database(&mut self, name: &str, records: Vec<(i64, Vec<u8>)>) -> &mut Self {
        let id = self.next_database;
        self.next_database += 1;
        self.entries.push(database_name("file__0@1", name, id));
        self.entries.push(store_name(id, 1, name));
        for (local_id, value) in records {
            self.entries
                .push(record(id, 1, &local_id.to_string(), &value));
        }
        self
    }

    fn write(&self, name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("uwussh-termius-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ops: Vec<Op> = self.entries.iter().map(|(k, v)| Op::Put(k, v)).collect();
        std::fs::write(dir.join("000003.log"), log(&[batch(1, &ops)])).unwrap();
        dir
    }
}

const SYNCED: &str = "SYNCHRONIZED";

fn homelab() -> Fixture {
    let mut fixture = Fixture::new();
    fixture
        .database(
            "groups",
            vec![
                (
                    1,
                    entity(
                        1,
                        SYNCED,
                        &[
                            ("label", Sealed("Homelab")),
                            ("parent_group", Null),
                            ("ssh_config", Ref(20)),
                        ],
                    ),
                ),
                (
                    2,
                    entity(
                        2,
                        SYNCED,
                        &[
                            ("label", Sealed("Proxmox")),
                            ("parent_group", Ref(1)),
                            ("ssh_config", Null),
                        ],
                    ),
                ),
            ],
        )
        .database(
            "ssh_configs",
            vec![
                (
                    10,
                    entity(10, SYNCED, &[("port", Num(22.0)), ("identity", Ref(31))]),
                ),
                (
                    11,
                    entity(
                        11,
                        SYNCED,
                        &[("identity", Null), ("env_variables", Sealed("{}"))],
                    ),
                ),
                (
                    12,
                    entity(12, SYNCED, &[("port", Num(70000.0)), ("identity", Ref(31))]),
                ),
                (
                    20,
                    entity(20, SYNCED, &[("port", Num(2222.0)), ("identity", Ref(30))]),
                ),
            ],
        )
        .database(
            "keys",
            vec![
                (
                    40,
                    entity(
                        40,
                        SYNCED,
                        &[
                            ("label", Sealed("nyu-key")),
                            (
                                "private_key",
                                Sealed(
                                    "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: aes256-cbc\n",
                                ),
                            ),
                            ("passphrase", Sealed("meow")),
                            ("public_key", Sealed("")),
                        ],
                    ),
                ),
                (
                    41,
                    entity(
                        41,
                        SYNCED,
                        &[
                            ("label", Sealed("mystery")),
                            ("private_key", Sealed("not a key")),
                        ],
                    ),
                ),
            ],
        )
        .database(
            "ssh_identities",
            vec![
                (
                    30,
                    entity(
                        30,
                        SYNCED,
                        &[
                            ("label", Sealed("root@homelab")),
                            ("username", Sealed("root")),
                            ("password", Sealed("")),
                            ("ssh_key", Ref(40)),
                            ("is_visible", Bool(true)),
                        ],
                    ),
                ),
                (
                    31,
                    entity(
                        31,
                        SYNCED,
                        &[
                            ("label", Sealed("")),
                            ("username", Sealed("uwu")),
                            ("password", Sealed("nyu")),
                            ("ssh_key", Null),
                            ("is_visible", Bool(false)),
                        ],
                    ),
                ),
                (
                    32,
                    entity(
                        32,
                        "DELETED",
                        &[("username", Sealed("old")), ("password", Sealed("gone"))],
                    ),
                ),
            ],
        )
        .database(
            "hosts",
            vec![
                (
                    50,
                    entity(
                        50,
                        SYNCED,
                        &[
                            ("label", Sealed("web")),
                            ("address", Sealed("10.0.0.5")),
                            ("group", Null),
                            ("ssh_config", Ref(10)),
                            ("os_name", Plain("debian")),
                        ],
                    ),
                ),
                // Only the `content` copy, no separate sealed fields.
                (
                    51,
                    entity(
                        51,
                        SYNCED,
                        &[
                            (
                                "content",
                                Sealed(r#"{"label":"pve-1","address":"pve-1.lan","version":1}"#),
                            ),
                            ("group", Ref(2)),
                            ("ssh_config", Ref(11)),
                        ],
                    ),
                ),
                (
                    52,
                    entity(
                        52,
                        "DELETED",
                        &[
                            ("label", Sealed("gone")),
                            ("address", Sealed("10.0.0.99")),
                            ("ssh_config", Ref(10)),
                        ],
                    ),
                ),
                (
                    53,
                    entity(
                        53,
                        SYNCED,
                        &[
                            ("label", Sealed("broken-port")),
                            ("address", Sealed("10.0.0.7")),
                            ("ssh_config", Ref(12)),
                        ],
                    ),
                ),
                (
                    54,
                    entity(
                        54,
                        SYNCED,
                        &[("label", Sealed("no-address")), ("ssh_config", Ref(10))],
                    ),
                ),
                (
                    55,
                    entity(
                        55,
                        SYNCED,
                        &[
                            ("label", SealedWith(TEAM_KEY, "team-box")),
                            ("address", SealedWith(TEAM_KEY, "10.1.0.1")),
                        ],
                    ),
                ),
            ],
        )
        .database(
            "tags",
            vec![(60, entity(60, SYNCED, &[("label", Sealed("prod"))]))],
        )
        .database(
            "tag_hosts",
            vec![(
                61,
                entity(61, SYNCED, &[("host", Ref(50)), ("tag", Ref(60))]),
            )],
        )
        .database(
            "known_hosts",
            vec![
                (
                    70,
                    entity(
                        70,
                        SYNCED,
                        &[
                            ("hostnames", Sealed("10.0.0.5")),
                            (
                                "key",
                                Sealed("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA== web"),
                            ),
                        ],
                    ),
                ),
                (
                    71,
                    entity(
                        71,
                        SYNCED,
                        &[
                            ("hostnames", Sealed("[pve-1.lan]:2222,pve-1")),
                            ("key", Sealed("ecdsa-sha2-nistp256 AAAAE2VjZHNh")),
                        ],
                    ),
                ),
                (
                    72,
                    entity(
                        72,
                        SYNCED,
                        &[
                            ("hostnames", Sealed("|1|c2FsdA==|aGFzaA==")),
                            ("key", Sealed("ssh-rsa AAAAB3NzaC1yc2E=")),
                        ],
                    ),
                ),
            ],
        )
        .database(
            "snippets_packages",
            vec![(80, entity(80, SYNCED, &[("label", Sealed("maintenance"))]))],
        )
        .database(
            "snippets",
            vec![
                (
                    81,
                    entity(
                        81,
                        SYNCED,
                        &[
                            ("label", Sealed("update")),
                            ("script", Sealed("apt update && apt upgrade")),
                            ("package", Ref(80)),
                        ],
                    ),
                ),
                (
                    82,
                    entity(
                        82,
                        SYNCED,
                        &[("label", Sealed("empty")), ("script", Sealed(""))],
                    ),
                ),
            ],
        );
    fixture
}

fn import_homelab(name: &str) -> ImportBundle {
    let dir = homelab().write(name);
    let bundle = import(&dir, &LocalKey::from_bytes(KEY)).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    bundle
}

fn skipped_reason<'a>(bundle: &'a ImportBundle, what: &str) -> Option<&'a str> {
    bundle
        .skipped
        .iter()
        .find(|(name, _)| name == what)
        .map(|(_, reason)| reason.as_str())
}

#[test]
fn hosts_arrive_with_address_port_group_and_login() {
    let bundle = import_homelab("hosts");
    let names: Vec<_> = bundle.hosts.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["web", "pve-1"]);

    let web = &bundle.hosts[0];
    assert_eq!((web.address.as_str(), web.port), ("10.0.0.5", 22));
    assert_eq!(web.group_path, None);
    assert_eq!(web.username.as_deref(), Some("uwu"));
    assert_eq!(web.tags, ["prod"]);
    assert_eq!(
        web.extras,
        [("termius.os_name".to_owned(), "debian".to_owned())]
    );
    let login = &bundle.identities[web.identity.unwrap()];
    assert_eq!(login.password.as_deref().map(String::as_str), Some("nyu"));
    assert!(!login.shared);
}

#[test]
fn a_host_inherits_port_and_login_from_its_groups() {
    let bundle = import_homelab("inherit");
    let pve = bundle.hosts.iter().find(|h| h.name == "pve-1").unwrap();

    assert_eq!(pve.address, "pve-1.lan", "read from the content copy");
    assert_eq!(pve.group_path.as_deref(), Some("Homelab/Proxmox"));
    assert_eq!(pve.port, 2222, "from the Homelab group's ssh config");
    assert_eq!(pve.username.as_deref(), Some("root"));

    let login = &bundle.identities[pve.identity.unwrap()];
    assert!(login.shared);
    assert!(login.password.is_none(), "an empty password is no password");
    let key = &bundle.keys[login.key.unwrap()];
    assert_eq!(key.label, "nyu-key");
    assert_eq!(key.format, KeyFormat::Ppk);
    assert_eq!(key.passphrase.as_deref().map(String::as_str), Some("meow"));
    assert_eq!(key.public_key, None);
}

#[test]
fn deleted_records_are_gone_and_problems_are_named() {
    let bundle = import_homelab("problems");

    assert!(bundle.hosts.iter().all(|h| h.name != "gone"));
    assert!(bundle
        .skipped
        .iter()
        .all(|(name, _)| !name.contains("gone")));
    assert!(bundle
        .identities
        .iter()
        .all(|i| i.username.as_deref() != Some("old")));
    assert_eq!(bundle.identities.len(), 2);

    assert_eq!(
        skipped_reason(&bundle, r#"host "broken-port""#),
        Some("has an invalid port")
    );
    assert_eq!(
        skipped_reason(&bundle, r#"host "no-address""#),
        Some("has no address")
    );
    assert_eq!(
        skipped_reason(&bundle, r#"key "mystery""#),
        Some("is in a format UwUSSH cannot read")
    );
    assert!(
        skipped_reason(&bundle, r#"host "unnamed""#).is_some_and(|r| r.contains("team vault")),
        "a host sealed with a team key is reported, not half-imported: {:?}",
        bundle.skipped
    );
    assert_eq!(bundle.keys.len(), 1);
}

#[test]
fn known_host_keys_come_along_per_host_and_port() {
    let bundle = import_homelab("known");
    let entries: Vec<_> = bundle
        .known_hosts
        .iter()
        .map(|k| (k.host.as_str(), k.port, k.algorithm.as_str()))
        .collect();
    assert_eq!(
        entries,
        [
            ("10.0.0.5", 22, "ssh-ed25519"),
            ("pve-1.lan", 2222, "ecdsa-sha2-nistp256"),
            ("pve-1", 22, "ecdsa-sha2-nistp256"),
        ]
    );
    assert_eq!(bundle.known_hosts[0].key, "AAAAC3NzaC1lZDI1NTE5AAAAIA==");
    assert_eq!(
        skipped_reason(&bundle, "a known host key"),
        Some("has only hashed host names")
    );
}

#[test]
fn snippets_keep_their_package_as_group() {
    let bundle = import_homelab("snippets");
    assert_eq!(
        bundle.snippets,
        [ImportedSnippet {
            label: "update".into(),
            script: "apt update && apt upgrade".into(),
            group: Some("maintenance".into()),
        }]
    );
}

#[test]
fn the_wrong_key_opens_nothing_and_says_so() {
    let dir = homelab().write("wrong-key");
    let bundle = import(&dir, &LocalKey::from_bytes([1; 32])).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(bundle.hosts.is_empty());
    assert!(bundle
        .skipped
        .iter()
        .any(|(_, reason)| reason.contains("team vault")));
}

#[test]
fn a_database_without_hosts_is_an_error() {
    let mut fixture = Fixture::new();
    fixture.database("snippets", vec![]);
    let dir = fixture.write("empty");
    let result = import(&dir, &LocalKey::from_bytes(KEY));
    std::fs::remove_dir_all(dir).unwrap();
    assert!(matches!(result, Err(TermiusError::NoHosts)));
}

#[test]
fn private_key_formats_are_recognised_by_their_first_line() {
    assert_eq!(
        KeyFormat::detect("-----BEGIN OPENSSH PRIVATE KEY-----\nb3Blbn...\n"),
        Some(KeyFormat::OpenSsh)
    );
    assert_eq!(
        KeyFormat::detect("\n-----BEGIN RSA PRIVATE KEY-----\r\nMIIE...\r\n"),
        Some(KeyFormat::Pem)
    );
    assert_eq!(
        KeyFormat::detect("PuTTY-User-Key-File-2: ssh-rsa\n"),
        Some(KeyFormat::Ppk)
    );
    assert_eq!(KeyFormat::detect("ssh-ed25519 AAAA… a public key"), None);
}
