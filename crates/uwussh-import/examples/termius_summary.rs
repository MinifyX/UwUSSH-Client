//! What a Termius import of this machine would bring — counts only.
//!
//!   cargo run -p uwussh-import --example termius_summary
//!
//! Runs the real importer against the local Termius install and reports how
//! many hosts, logins, keys, known hosts and snippets it found, whether every
//! key opens with its passphrase in russh, and why anything was skipped —
//! without a single host name, address or secret in the output.

use std::collections::BTreeMap;
use uwussh_import::termius;

fn main() {
    let bundle = match termius::import_local() {
        Ok(bundle) => bundle,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let hosts = &bundle.hosts;
    let count =
        |f: &dyn Fn(&uwussh_import::ImportedHost) -> bool| hosts.iter().filter(|h| f(h)).count();
    println!("hosts            {}", hosts.len());
    println!("  in a group     {}", count(&|h| h.group_path.is_some()));
    println!("  with a login   {}", count(&|h| h.identity.is_some()));
    println!("  with username  {}", count(&|h| h.username.is_some()));
    println!(
        "  password       {}",
        count(&|h| h
            .identity
            .is_some_and(|i| bundle.identities[i].password.is_some()))
    );
    println!(
        "  key            {}",
        count(&|h| h
            .identity
            .is_some_and(|i| bundle.identities[i].key.is_some()))
    );
    println!("  port ≠ 22      {}", count(&|h| h.port != 22));
    println!("  tagged         {}", count(&|h| !h.tags.is_empty()));

    let shared = bundle.identities.iter().filter(|i| i.shared).count();
    println!(
        "logins           {} ({shared} from the keychain)",
        bundle.identities.len()
    );

    for (n, key) in bundle.keys.iter().enumerate() {
        let opened = russh::keys::decode_secret_key(
            &key.private_key,
            key.passphrase.as_ref().map(|p| p.as_str()),
        );
        match opened {
            Ok(private) => println!(
                "key {n}            {:?}, opens with its passphrase: yes, {}",
                key.format,
                private.algorithm().as_str()
            ),
            Err(e) => println!("key {n}            {:?}, opens: NO ({e})", key.format),
        }
    }

    let mut algorithms = BTreeMap::<&str, usize>::new();
    for known in &bundle.known_hosts {
        *algorithms.entry(known.algorithm.as_str()).or_default() += 1;
    }
    println!(
        "known host keys  {} {algorithms:?}",
        bundle.known_hosts.len()
    );
    println!(
        "snippets         {} ({} in a package)",
        bundle.snippets.len(),
        bundle.snippets.iter().filter(|s| s.group.is_some()).count()
    );

    let mut reasons = BTreeMap::<&str, usize>::new();
    for (_, reason) in &bundle.skipped {
        *reasons.entry(reason.as_str()).or_default() += 1;
    }
    println!("skipped          {reasons:?}");
}
