//! Seed a database with one host that logs in with a key from the vault.
//!
//! ```text
//! cargo run -p uwussh-core --example seed_vault_key -- \
//!   <db> <master-password> <name> <address> <port> <username> <authorized-key-out>
//! ```
//!
//! Generates a fresh ed25519 key, creates the vault, imports one host whose
//! login is that key, and writes the matching public key to
//! `<authorized-key-out>` so `dev_sshd` can authorize it. The end-to-end run
//! uses this to test connecting with a vault key against a real server: after
//! this, the on-disk vault is locked, exactly as the app finds it on start.
//!
//! It lives in `uwussh-core` next to `dev_sshd` because both are end-to-end
//! fixtures and a real key is already generated here.

use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, PrivateKey};
use std::path::PathBuf;
use uwussh_store::{HostInput, IdentityInput, ImportSet, KeyInput, Store};
use zeroize::Zeroizing;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [db, password, name, address, port, username, key_out] = args.as_slice() else {
        eprintln!(
            "usage: seed_vault_key <db> <master-password> <name> <address> <port> <username> <authorized-key-out>"
        );
        std::process::exit(2);
    };
    let port: u16 = port.parse().expect("port");

    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("key generation");
    let private_pem = key
        .to_openssh(LineEnding::LF)
        .expect("encode private")
        .to_string();
    let public_line = key.public_key().to_openssh().expect("encode public");

    let store = Store::open(&PathBuf::from(db)).expect("open store");
    store
        .create_vault(password.as_bytes())
        .expect("create vault");

    let outcome = store
        .import(ImportSet {
            keys: vec![KeyInput {
                label: format!("{name} key"),
                key_type: "ed25519".into(),
                public_key: Some(public_line.clone()),
                private_key: Zeroizing::new(private_pem),
                passphrase: None,
            }],
            identities: vec![IdentityInput {
                label: Some(username.clone()),
                username: Some(username.clone()),
                password: None,
                key: Some(0),
                key_path: None,
            }],
            hosts: vec![HostInput {
                name: name.clone(),
                address: address.clone(),
                port,
                group_path: None,
                identity: Some(0),
                workspace: Default::default(),
                position: None,
            }],
            ..Default::default()
        })
        .expect("import");

    // Lock again, so the file is exactly what the app opens: a vault at rest.
    store.lock_vault();

    std::fs::write(key_out, format!("{public_line}\n")).expect("write authorized key");
    println!(
        "seeded {} host(s) into {db}; authorized key in {key_out}",
        outcome.hosts_added
    );
}
