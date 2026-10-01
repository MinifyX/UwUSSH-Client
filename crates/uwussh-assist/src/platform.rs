//! The system a command is for: which family, which shell, which package
//! manager. Made from what the connection found (`uwussh_core::os`) or from
//! this machine, and part of every cache key — a command for bash is never
//! offered to PowerShell, nor one for apt to a system with dnf.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    Linux,
    Macos,
    Bsd,
    Windows,
    /// Cisco IOS and its relatives: no shell, a command line of its own.
    CiscoIos,
    /// MikroTik RouterOS.
    Routeros,
    /// Nothing was found out; something Unix-like is the best guess.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    /// A plain POSIX shell.
    Sh,
    /// BusyBox's ash with BusyBox's tools, as on Alpine and OpenWrt.
    Busybox,
    Powershell,
    Cmd,
    Ios,
    Routeros,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Packages {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Apk,
    Opkg,
    Nix,
    Brew,
    Pkg,
    Winget,
    None,
}

/// Everything the prompt and the cache need to know about the target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Platform {
    pub family: Family,
    pub shell: Shell,
    pub packages: Packages,
    /// The system as `uwussh_core::os` names it (`ubuntu`, `proxmox`, …), for
    /// the prompt only. Not part of the key: Ubuntu and Debian share answers.
    pub os: Option<String>,
}

impl Platform {
    /// The platform of a system known by its id from the OS detection, or a
    /// generic Unix one when nothing was found.
    pub fn for_os(os: Option<&str>) -> Self {
        let id = os.map(|os| os.trim().to_ascii_lowercase());
        let (family, shell, packages) = match id.as_deref() {
            Some(
                "ubuntu" | "debian" | "raspberry" | "mint" | "kali" | "proxmox" | "pop"
                | "elementary",
            ) => (Family::Linux, Shell::Bash, Packages::Apt),
            Some("fedora" | "redhat" | "centos" | "rocky" | "alma") => {
                (Family::Linux, Shell::Bash, Packages::Dnf)
            }
            Some("arch" | "manjaro") => (Family::Linux, Shell::Bash, Packages::Pacman),
            Some("suse") => (Family::Linux, Shell::Bash, Packages::Zypper),
            Some("nixos") => (Family::Linux, Shell::Bash, Packages::Nix),
            Some("alpine") => (Family::Linux, Shell::Busybox, Packages::Apk),
            Some("openwrt") => (Family::Linux, Shell::Busybox, Packages::Opkg),
            Some("synology" | "linux") => (Family::Linux, Shell::Bash, Packages::None),
            Some("macos") => (Family::Macos, Shell::Zsh, Packages::Brew),
            Some("freebsd") => (Family::Bsd, Shell::Sh, Packages::Pkg),
            Some("windows") => (Family::Windows, Shell::Powershell, Packages::Winget),
            Some("cisco") => (Family::CiscoIos, Shell::Ios, Packages::None),
            Some("mikrotik") => (Family::Routeros, Shell::Routeros, Packages::None),
            _ => (Family::Unknown, Shell::Sh, Packages::None),
        };
        Self {
            family,
            shell,
            packages,
            os: id.filter(|id| !id.is_empty()),
        }
    }

    /// Another shell on the same system, when it fits that system: zsh on a
    /// Linux box, cmd instead of PowerShell. Anything else is ignored.
    pub fn with_shell(mut self, shell: Shell) -> Self {
        if self.shells().contains(&shell) {
            self.shell = shell;
        }
        self
    }

    /// The login shell's path or name (`/usr/bin/zsh`, `pwsh.exe`) as a shell.
    pub fn shell_from_path(path: &str) -> Option<Shell> {
        let name = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path)
            .trim()
            .to_ascii_lowercase();
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        Some(match name {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            "sh" | "dash" | "ksh" | "mksh" => Shell::Sh,
            "ash" | "busybox" => Shell::Busybox,
            "powershell" | "pwsh" => Shell::Powershell,
            "cmd" => Shell::Cmd,
            _ => return None,
        })
    }

    /// The shells the popup offers to switch between on this system.
    pub fn shells(&self) -> Vec<Shell> {
        match self.family {
            Family::Windows => vec![Shell::Powershell, Shell::Cmd],
            Family::CiscoIos => vec![Shell::Ios],
            Family::Routeros => vec![Shell::Routeros],
            Family::Linux | Family::Macos | Family::Bsd | Family::Unknown => vec![
                Shell::Bash,
                Shell::Zsh,
                Shell::Fish,
                Shell::Sh,
                Shell::Busybox,
            ],
        }
    }

    /// The cache key: family, shell and package manager, like
    /// `linux/bash/apt`.
    pub fn key(&self) -> String {
        format!(
            "{}/{}/{}",
            name(self.family),
            name(self.shell),
            name(self.packages)
        )
    }

    /// The target, in words for the model.
    pub fn describe(&self) -> String {
        let system = match self.family {
            Family::Linux => match self.os.as_deref() {
                Some(os) if os != "linux" => format!("Linux ({os})"),
                _ => "Linux".to_string(),
            },
            Family::Macos => "macOS with the BSD userland (not GNU coreutils)".to_string(),
            Family::Bsd => "FreeBSD with the BSD userland".to_string(),
            Family::Windows => "Windows".to_string(),
            Family::CiscoIos => "a Cisco IOS network device (privileged EXEC mode)".to_string(),
            Family::Routeros => "a MikroTik RouterOS device".to_string(),
            Family::Unknown => "an unknown Unix-like system; stay POSIX".to_string(),
        };
        let shell = match self.shell {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
            Shell::Sh => "a POSIX sh",
            Shell::Busybox => "BusyBox ash with BusyBox applets (limited options)",
            Shell::Powershell => "PowerShell",
            Shell::Cmd => "cmd.exe",
            Shell::Ios => "the IOS command line",
            Shell::Routeros => "the RouterOS command line",
        };
        let packages = match self.packages {
            Packages::Apt => Some("apt"),
            Packages::Dnf => Some("dnf"),
            Packages::Pacman => Some("pacman"),
            Packages::Zypper => Some("zypper"),
            Packages::Apk => Some("apk"),
            Packages::Opkg => Some("opkg"),
            Packages::Nix => Some("nix"),
            Packages::Brew => Some("Homebrew (brew)"),
            Packages::Pkg => Some("pkg"),
            Packages::Winget => Some("winget"),
            Packages::None => None,
        };
        match packages {
            Some(packages) => format!("{system}, shell: {shell}, package manager: {packages}"),
            None => format!("{system}, shell: {shell}"),
        }
    }
}

/// The kebab-case name serde gives a unit variant.
fn name<T: Serialize>(value: T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distributions_get_their_package_manager() {
        assert_eq!(Platform::for_os(Some("ubuntu")).key(), "linux/bash/apt");
        assert_eq!(Platform::for_os(Some("proxmox")).key(), "linux/bash/apt");
        assert_eq!(Platform::for_os(Some("rocky")).key(), "linux/bash/dnf");
        assert_eq!(Platform::for_os(Some("arch")).key(), "linux/bash/pacman");
        assert_eq!(Platform::for_os(Some("alpine")).key(), "linux/busybox/apk");
        assert_eq!(
            Platform::for_os(Some("openwrt")).key(),
            "linux/busybox/opkg"
        );
    }

    #[test]
    fn other_systems_get_their_own_command_line() {
        assert_eq!(Platform::for_os(Some("macos")).key(), "macos/zsh/brew");
        assert_eq!(Platform::for_os(Some("freebsd")).key(), "bsd/sh/pkg");
        assert_eq!(
            Platform::for_os(Some("windows")).key(),
            "windows/powershell/winget"
        );
        assert_eq!(Platform::for_os(Some("cisco")).key(), "cisco-ios/ios/none");
        assert_eq!(
            Platform::for_os(Some("mikrotik")).key(),
            "routeros/routeros/none"
        );
        assert_eq!(Platform::for_os(None).key(), "unknown/sh/none");
        assert!(Platform::for_os(Some("macos")).describe().contains("BSD"));
    }

    #[test]
    fn a_shell_is_switched_only_where_it_exists() {
        let windows = Platform::for_os(Some("windows"));
        assert_eq!(windows.clone().with_shell(Shell::Cmd).shell, Shell::Cmd);
        assert_eq!(
            windows.with_shell(Shell::Zsh).shell,
            Shell::Powershell,
            "no zsh on Windows"
        );
        let ubuntu = Platform::for_os(Some("ubuntu")).with_shell(Shell::Zsh);
        assert_eq!(ubuntu.key(), "linux/zsh/apt");
        assert_ne!(ubuntu.key(), Platform::for_os(Some("ubuntu")).key());
    }

    #[test]
    fn login_shells_are_read_from_their_path() {
        assert_eq!(Platform::shell_from_path("/usr/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(Platform::shell_from_path("/bin/bash"), Some(Shell::Bash));
        assert_eq!(
            Platform::shell_from_path("C:\\Windows\\powershell.exe"),
            Some(Shell::Powershell)
        );
        assert_eq!(Platform::shell_from_path("pwsh"), Some(Shell::Powershell));
        assert_eq!(Platform::shell_from_path("/usr/bin/xonsh"), None);
    }
}
