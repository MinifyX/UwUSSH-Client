//! What a server runs, found out the way Termius does: by asking it.
//!
//! Right after the terminal opens, a second channel on the same connection runs
//! one harmless command line — `uname`, `/etc/os-release` and a few marker
//! files — and the answer is matched against known systems. The server's SSH
//! banner decides first where a shell can't answer: Cisco and MikroTik devices
//! often refuse `exec`, but name themselves in the banner.
//!
//! The result is only a picture in the host list. Nothing trusts it, so a
//! server lying about itself gets a wrong icon and nothing else.

/// The command line sent to the server. POSIX `sh`; on Windows, cmd and
/// PowerShell fail on it in a recognisable way, which is an answer too.
pub const PROBE_COMMAND: &str = "uname -s 2>/dev/null; cat /etc/os-release 2>/dev/null; \
[ -d /etc/pve ] && echo UWUSSH-PROXMOX; \
[ -f /etc/synoinfo.conf ] && echo UWUSSH-SYNOLOGY; \
[ -f /etc/openwrt_release ] && echo UWUSSH-OPENWRT; \
cat /proc/device-tree/model 2>/dev/null; echo";

/// The most output a probe reads; a real answer is a few hundred bytes.
pub const PROBE_LIMIT: usize = 16 * 1024;

/// Systems known from the server's SSH identification string alone.
pub fn from_banner(banner: &str) -> Option<&'static str> {
    let banner = banner.to_ascii_lowercase();
    if banner.contains("cisco") {
        Some("cisco")
    } else if banner.contains("rosssh") {
        Some("mikrotik")
    } else if banner.contains("for_windows") {
        Some("windows")
    } else {
        None
    }
}

/// The system in a probe's output, or `None` when nothing matched.
pub fn from_probe(output: &str) -> Option<&'static str> {
    let lower = output.to_ascii_lowercase();
    if lower.contains("is not recognized as") || lower.contains("cannot find the path specified") {
        return Some("windows");
    }
    // Markers from the probe itself outrank what os-release says: Proxmox is
    // Debian underneath, a Raspberry Pi usually is too.
    for (marker, os) in [
        ("uwussh-proxmox", "proxmox"),
        ("uwussh-synology", "synology"),
        ("uwussh-openwrt", "openwrt"),
        ("raspberry pi", "raspberry"),
    ] {
        if lower.contains(marker) {
            return Some(os);
        }
    }

    let mut id = None;
    let mut like = None;
    let mut kernel = None;
    for line in output.lines() {
        let line = line.trim().trim_matches('\0');
        if let Some(value) = line.strip_prefix("ID=") {
            id = Some(unquote(value));
        } else if let Some(value) = line.strip_prefix("ID_LIKE=") {
            like = Some(unquote(value));
        } else if kernel.is_none() && !line.is_empty() && !line.contains('=') {
            kernel = Some(line.to_ascii_lowercase());
        }
    }

    if let Some(os) = id.as_deref().and_then(by_id) {
        return Some(os);
    }
    if let Some(like) = like {
        for word in like.split_whitespace() {
            if let Some(os) = by_like(word) {
                return Some(os);
            }
        }
    }
    match kernel.as_deref() {
        Some("darwin") => Some("macos"),
        Some("freebsd") => Some("freebsd"),
        Some("linux") => Some("linux"),
        _ => None,
    }
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_ascii_lowercase()
}

fn by_id(id: &str) -> Option<&'static str> {
    Some(match id {
        "ubuntu" | "pop" | "elementary" | "zorin" | "neon" => "ubuntu",
        "debian" => "debian",
        "raspbian" => "raspberry",
        "fedora" => "fedora",
        "rhel" | "ol" | "amzn" => "redhat",
        "centos" => "centos",
        "rocky" => "rocky",
        "almalinux" => "alma",
        "arch" | "manjaro" | "endeavouros" | "archarm" => "arch",
        "alpine" => "alpine",
        "opensuse" | "opensuse-leap" | "opensuse-tumbleweed" | "sles" | "sled" => "suse",
        "linuxmint" => "mint",
        "kali" => "kali",
        "nixos" => "nixos",
        "freebsd" => "freebsd",
        "openwrt" => "openwrt",
        _ => return None,
    })
}

fn by_like(word: &str) -> Option<&'static str> {
    Some(match word {
        "ubuntu" => "ubuntu",
        "debian" => "debian",
        "rhel" | "fedora" | "centos" => "redhat",
        "arch" => "arch",
        "suse" | "opensuse" => "suse",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distributions_are_told_apart_by_os_release() {
        let ubuntu = "Linux\nPRETTY_NAME=\"Ubuntu 24.04 LTS\"\nID=ubuntu\nID_LIKE=debian\n";
        assert_eq!(from_probe(ubuntu), Some("ubuntu"));
        let rocky = "Linux\nNAME=\"Rocky Linux\"\nID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n";
        assert_eq!(from_probe(rocky), Some("rocky"));
        let unknown_rhel_like = "Linux\nID=\"eurolinux\"\nID_LIKE=\"rhel fedora\"\n";
        assert_eq!(from_probe(unknown_rhel_like), Some("redhat"));
        assert_eq!(from_probe("Linux\n"), Some("linux"));
    }

    #[test]
    fn markers_beat_the_distribution_underneath() {
        let proxmox = "Linux\nID=debian\nUWUSSH-PROXMOX\n";
        assert_eq!(from_probe(proxmox), Some("proxmox"));
        let pi = "Linux\nID=debian\nRaspberry Pi 4 Model B Rev 1.4\0\n";
        assert_eq!(from_probe(pi), Some("raspberry"));
    }

    #[test]
    fn mac_bsd_and_windows_answer_in_their_own_way() {
        assert_eq!(from_probe("Darwin\n\n"), Some("macos"));
        assert_eq!(from_probe("FreeBSD\n"), Some("freebsd"));
        let cmd = "'uname' is not recognized as an internal or external command,\r\noperable program or batch file.\r\n";
        assert_eq!(from_probe(cmd), Some("windows"));
        assert_eq!(from_probe(""), None);
    }

    #[test]
    fn network_gear_is_known_by_its_banner() {
        assert_eq!(from_banner("SSH-2.0-Cisco-1.25"), Some("cisco"));
        assert_eq!(from_banner("SSH-2.0-ROSSSH"), Some("mikrotik"));
        assert_eq!(
            from_banner("SSH-2.0-OpenSSH_for_Windows_9.5"),
            Some("windows")
        );
        assert_eq!(from_banner("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3"), None);
    }
}
