//! Several accounts, one team server, no network.
//!
//! The server here follows the rules of UwUSync's `docs/teams.md` — and can
//! be made to break them, the way one that was taken over would: list a
//! public key nobody confirmed, hand out a key of its own making, hand out
//! an old one again.

use crate::engine::Transport;
use crate::http::{decode_base64, encode_base64};
use crate::team::{
    MemberState, Policy, RotateKey, TeamApi, TeamError, TeamPull, Teams, WireIdentity, WireInvite,
    WireJoined, WireMember, WireTeam,
};
use crate::MemoryServer;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;
use uwussh_proto::{Envelope, PushResponse, SyncCursor};
use uwussh_store::{
    AuthMethod, HostDraft, PasswordChange, SecretText, SpaceHostDraft, Spaces, TeamRole,
};
use uwussh_vault::team::{new_team_key, wrap_team_key};
use uwussh_vault::{KdfParams, WrapContext};

type Result<T> = std::result::Result<T, TeamError>;

fn refused<T>(code: &str) -> Result<T> {
    Err(TeamError::Refused {
        code: code.into(),
        message: code.into(),
        epoch: None,
    })
}

struct Member {
    account: Uuid,
    display: String,
    role: TeamRole,
    state: MemberState,
    public: String,
    wrapped: Option<String>,
    /// The epoch the wrap is handed out as. The team's, unless the server
    /// was told to lie.
    epoch: Option<u64>,
}

struct Team {
    name: String,
    epoch: u64,
    needs_rotation: bool,
    members: Vec<Member>,
    mailbox: Arc<MemoryServer>,
}

#[derive(Default)]
struct State {
    old: bool,
    disabled: bool,
    down: bool,
    /// Do the next rotation, and lose the answer.
    drop_rotate_answer: bool,
    /// Refuse the next rotation with this.
    refuse_rotate: Option<&'static str>,
    rotations: u32,
    identities: BTreeMap<Uuid, (String, String)>,
    teams: BTreeMap<Uuid, Team>,
    invites: BTreeMap<String, (Uuid, Uuid, TeamRole)>,
}

#[derive(Default)]
struct Fake {
    state: Mutex<State>,
}

impl Fake {
    fn team<R>(&self, team: Uuid, f: impl FnOnce(&mut Team) -> R) -> R {
        f(self.state.lock().teams.get_mut(&team).expect("a team"))
    }

    fn epoch(&self, team: Uuid) -> u64 {
        self.team(team, |team| team.epoch)
    }

    fn mailbox(&self, team: Uuid) -> Arc<MemoryServer> {
        self.team(team, |team| team.mailbox.clone())
    }

    fn wrapped(&self, team: Uuid, account: Uuid) -> Option<String> {
        self.team(team, |team| {
            team.members
                .iter()
                .find(|member| member.account == account)
                .and_then(|member| member.wrapped.clone())
        })
    }
}

struct As<'a> {
    fake: &'a Fake,
    account: Uuid,
}

impl As<'_> {
    fn with<R>(&self, f: impl FnOnce(&mut State) -> Result<R>) -> Result<R> {
        let mut state = self.fake.state.lock();
        if state.down {
            return Err(TeamError::Offline("down".into()));
        }
        if state.old || state.disabled {
            return Err(TeamError::Unsupported);
        }
        f(&mut state)
    }

    /// The team and this account's place in it, when it is active there.
    fn active<'s>(&self, state: &'s mut State, team: Uuid) -> Result<(&'s mut Team, TeamRole)> {
        let found = state.teams.get_mut(&team);
        let Some(found) = found else {
            return refused("not-found");
        };
        let me = found
            .members
            .iter()
            .find(|member| member.account == self.account);
        match me {
            None => refused("not-found"),
            Some(member) if member.state != MemberState::Active => refused("membership-pending"),
            Some(member) => {
                let role = member.role;
                Ok((found, role))
            }
        }
    }

    fn managing<'s>(&self, state: &'s mut State, team: Uuid) -> Result<&'s mut Team> {
        let (team, role) = self.active(state, team)?;
        if !role.manages() {
            return refused("forbidden");
        }
        Ok(team)
    }

    fn wire(&self, id: Uuid, team: &Team) -> Option<WireTeam> {
        let me = team
            .members
            .iter()
            .find(|member| member.account == self.account)?;
        Some(WireTeam {
            id,
            name: team.name.clone(),
            role: me.role,
            state: me.state,
            epoch: me.epoch.unwrap_or(team.epoch),
            wrapped_key: me.wrapped.clone(),
            needs_rotation: team.needs_rotation,
            members: team.members.len() as u32,
        })
    }
}

impl TeamApi for As<'_> {
    fn account(&self) -> Option<Uuid> {
        Some(self.account)
    }

    fn policy(&self) -> Result<Policy> {
        let state = self.fake.state.lock();
        if state.down {
            return Err(TeamError::Offline("down".into()));
        }
        if state.old {
            return Err(TeamError::Unsupported);
        }
        Ok(Policy {
            enabled: !state.disabled,
            may_create: true,
        })
    }

    fn identity(&self) -> Result<Option<WireIdentity>> {
        self.with(|state| {
            Ok(state
                .identities
                .get(&self.account)
                .map(|(public_key, sealed_private)| WireIdentity {
                    public_key: public_key.clone(),
                    sealed_private: sealed_private.clone(),
                }))
        })
    }

    fn put_identity(&self, public_key: &str, sealed_private: &str) -> Result<WireIdentity> {
        self.with(|state| {
            if state.identities.contains_key(&self.account) {
                return refused("identity-exists");
            }
            state
                .identities
                .insert(self.account, (public_key.into(), sealed_private.into()));
            Ok(WireIdentity {
                public_key: public_key.into(),
                sealed_private: sealed_private.into(),
            })
        })
    }

    fn teams(&self) -> Result<Vec<WireTeam>> {
        self.with(|state| {
            Ok(state
                .teams
                .iter()
                .filter_map(|(id, team)| self.wire(*id, team))
                .collect())
        })
    }

    fn create(&self, name: &str, display_name: &str, wrapped_key: &str) -> Result<WireTeam> {
        self.with(|state| {
            let Some((public, _)) = state.identities.get(&self.account).cloned() else {
                return refused("no-identity");
            };
            let id = Uuid::now_v7();
            let team = Team {
                name: name.into(),
                epoch: 1,
                needs_rotation: false,
                members: vec![Member {
                    account: self.account,
                    display: display_name.into(),
                    role: TeamRole::Owner,
                    state: MemberState::Active,
                    public,
                    wrapped: Some(wrapped_key.into()),
                    epoch: None,
                }],
                mailbox: Arc::new(MemoryServer::new()),
            };
            let wire = self.wire(id, &team).unwrap();
            state.teams.insert(id, team);
            Ok(wire)
        })
    }

    fn rename(&self, team: Uuid, name: &str) -> Result<WireTeam> {
        self.with(|state| {
            let found = self.managing(state, team)?;
            found.name = name.into();
            Ok(self.wire(team, found).unwrap())
        })
    }

    fn delete(&self, team: Uuid) -> Result<()> {
        self.with(|state| {
            let (_, role) = self.active(state, team)?;
            if role != TeamRole::Owner {
                return refused("forbidden");
            }
            state.teams.remove(&team);
            Ok(())
        })
    }

    fn invite(&self, team: Uuid, role: TeamRole) -> Result<WireInvite> {
        self.with(|state| {
            self.managing(state, team)?;
            let id = Uuid::now_v7();
            let code = format!("CODE-{}", state.invites.len() + 1);
            state.invites.insert(code.clone(), (team, id, role));
            Ok(WireInvite {
                id,
                role,
                expires_ms: 1_790_604_800_000,
                code: Some(code),
            })
        })
    }

    fn invites(&self, team: Uuid) -> Result<Vec<WireInvite>> {
        self.with(|state| {
            self.managing(state, team)?;
            Ok(state
                .invites
                .values()
                .filter(|(of, _, _)| *of == team)
                .map(|(_, id, role)| WireInvite {
                    id: *id,
                    role: *role,
                    expires_ms: 1_790_604_800_000,
                    code: None,
                })
                .collect())
        })
    }

    fn revoke_invite(&self, team: Uuid, invite: Uuid) -> Result<()> {
        self.with(|state| {
            self.managing(state, team)?;
            state.invites.retain(|_, (_, id, _)| *id != invite);
            Ok(())
        })
    }

    fn join(&self, code: &str, display_name: &str) -> Result<WireJoined> {
        self.with(|state| {
            let Some((public, _)) = state.identities.get(&self.account).cloned() else {
                return refused("no-identity");
            };
            let Some((team, _, role)) = state.invites.remove(code) else {
                return refused("invalid-invite");
            };
            let found = state.teams.get_mut(&team).unwrap();
            found.members.push(Member {
                account: self.account,
                display: display_name.into(),
                role,
                state: MemberState::Pending,
                public,
                wrapped: None,
                epoch: None,
            });
            Ok(WireJoined {
                team_id: team,
                name: found.name.clone(),
                role,
            })
        })
    }

    fn members(&self, team: Uuid) -> Result<Vec<WireMember>> {
        self.with(|state| {
            let (found, _) = self.active(state, team)?;
            Ok(found
                .members
                .iter()
                .map(|member| WireMember {
                    account_id: member.account,
                    display_name: member.display.clone(),
                    role: member.role,
                    state: member.state,
                    public_key: member.public.clone(),
                    joined_ms: 1,
                })
                .collect())
        })
    }

    fn confirm(
        &self,
        team: Uuid,
        account: Uuid,
        epoch: u64,
        public_key: &str,
        wrapped_key: &str,
    ) -> Result<()> {
        self.with(|state| {
            let found = self.managing(state, team)?;
            if epoch != found.epoch {
                return refused("stale-epoch");
            }
            let Some(member) = found.members.iter_mut().find(|m| m.account == account) else {
                return refused("not-found");
            };
            if member.public != public_key {
                return refused("identity-changed");
            }
            member.wrapped = Some(wrapped_key.into());
            member.state = MemberState::Active;
            Ok(())
        })
    }

    fn set_role(&self, team: Uuid, account: Uuid, role: TeamRole) -> Result<()> {
        self.with(|state| {
            let found = self.managing(state, team)?;
            let Some(member) = found.members.iter_mut().find(|m| m.account == account) else {
                return refused("not-found");
            };
            member.role = role;
            Ok(())
        })
    }

    fn remove_member(&self, team: Uuid, account: Uuid) -> Result<()> {
        self.with(|state| {
            let found = if account == self.account {
                self.active(state, team)?.0
            } else {
                self.managing(state, team)?
            };
            let Some(at) = found.members.iter().position(|m| m.account == account) else {
                return refused("not-found");
            };
            let owners = found
                .members
                .iter()
                .filter(|m| m.role == TeamRole::Owner && m.state == MemberState::Active)
                .count();
            if found.members[at].role == TeamRole::Owner && owners == 1 {
                return refused("last-owner");
            }
            let gone = found.members.remove(at);
            found.needs_rotation |= gone.wrapped.is_some();
            Ok(())
        })
    }

    fn rotate(&self, team: Uuid, epoch: u64, keys: &[RotateKey], cursor: u64) -> Result<WireTeam> {
        self.with(|state| {
            if let Some(code) = state.refuse_rotate.take() {
                return refused(code);
            }
            let lose = std::mem::take(&mut state.drop_rotate_answer);
            let found = self.managing(state, team)?;
            if epoch != found.epoch + 1 {
                return Err(TeamError::Refused {
                    code: "stale-epoch".into(),
                    message: String::new(),
                    epoch: Some(found.epoch),
                });
            }
            let mut active: Vec<Uuid> = found
                .members
                .iter()
                .filter(|m| m.state == MemberState::Active)
                .map(|m| m.account)
                .collect();
            let mut named: Vec<Uuid> = keys.iter().map(|key| key.account_id).collect();
            active.sort();
            named.sort();
            if active != named {
                return refused("members-mismatch");
            }
            if cursor != found.mailbox.cursor() {
                return refused("stale-cursor");
            }
            for member in &mut found.members {
                if let Some(key) = keys.iter().find(|key| key.account_id == member.account) {
                    member.wrapped = Some(key.wrapped_key.clone());
                }
            }
            found.epoch = epoch;
            found.needs_rotation = false;
            found.mailbox = Arc::new(MemoryServer::starting_at(found.mailbox.cursor()));
            let wire = self.wire(team, found).unwrap();
            state.rotations += 1;
            if lose {
                return Err(TeamError::Offline("the answer was lost".into()));
            }
            Ok(wire)
        })
    }

    fn pull(&self, team: Uuid, cursor: u64, limit: usize) -> Result<TeamPull> {
        self.with(|state| {
            let (found, role) = self.active(state, team)?;
            let page = found
                .mailbox
                .pull(SyncCursor(cursor), limit)
                .map_err(TeamError::from)?;
            Ok(TeamPull {
                page,
                epoch: found.epoch,
                role,
                needs_rotation: found.needs_rotation,
            })
        })
    }

    fn push(&self, team: Uuid, epoch: u64, envelopes: Vec<Envelope>) -> Result<PushResponse> {
        self.with(|state| {
            let (found, role) = self.active(state, team)?;
            if !role.writes() {
                return refused("forbidden");
            }
            if epoch != found.epoch {
                return refused("stale-epoch");
            }
            found.mailbox.push(envelopes).map_err(TeamError::from)
        })
    }
}

// ── Devices ───────────────────────────────────────────────────────────────────

const PASSWORD: &[u8] = b"correct horse battery staple";

struct Device {
    folder: PathBuf,
    spaces: Spaces,
    account: Uuid,
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn opened() -> (PathBuf, Spaces) {
    let folder = std::env::temp_dir().join(format!("uwussh-teams-{}", Uuid::now_v7()));
    let spaces = Spaces::open(&folder.join("uwussh.db")).unwrap();
    (folder, spaces)
}

impl Device {
    /// A device of an account of its own.
    fn new() -> Self {
        let (folder, spaces) = opened();
        spaces
            .main()
            .create_vault_with(PASSWORD, KdfParams::INSECURE_FOR_TESTS)
            .unwrap();
        Self {
            folder,
            spaces,
            account: Uuid::now_v7(),
        }
    }

    /// Another device of the same account: the same vault, paired.
    fn second(&self) -> Self {
        let header = self.spaces.main().vault_header().unwrap().unwrap();
        let key = header.unlock(PASSWORD).unwrap().export_key();
        let (folder, spaces) = opened();
        spaces.main().adopt_vault(&header, key).unwrap();
        Self {
            folder,
            spaces,
            account: self.account,
        }
    }

    fn on<R>(&self, fake: &Fake, f: impl FnOnce(&Teams<'_, As<'_>>) -> R) -> R {
        let api = As {
            fake,
            account: self.account,
        };
        let server = self.spaces.main_id();
        f(&Teams::new(&self.spaces, &server, &api))
    }

    fn pass(&self, fake: &Fake) -> crate::TeamPass {
        self.on(fake, |teams| teams.pass()).unwrap()
    }

    fn fingerprint(&self, fake: &Fake) -> String {
        self.pass(fake).status.fingerprint.expect("a member key")
    }

    fn space(&self, team: Uuid) -> Option<String> {
        self.spaces
            .team_space(&self.spaces.main_id(), team)
            .map(|space| space.id)
    }

    fn add(&self, team: Uuid, name: &str) -> std::result::Result<Uuid, uwussh_store::StoreError> {
        self.add_with(team, name, PasswordChange::Keep)
    }

    fn add_with(
        &self,
        team: Uuid,
        name: &str,
        password: PasswordChange,
    ) -> std::result::Result<Uuid, uwussh_store::StoreError> {
        let draft = HostDraft {
            id: None,
            name: name.into(),
            address: "192.0.2.10".into(),
            port: 22,
            username: "uwu".into(),
            auth: AuthMethod::Password,
            key_path: None,
            group_path: None,
            workspace: None,
            key_id: None,
            password,
        };
        self.spaces
            .save_host(SpaceHostDraft {
                draft,
                space: Some(self.space(team).expect("the team's space")),
            })
            .map(|host| host.host.id)
    }

    /// The names of the hosts the team has here.
    fn hosts(&self, team: Uuid) -> Vec<String> {
        let Some(space) = self.space(team) else {
            return Vec::new();
        };
        let mut names: Vec<String> = self
            .spaces
            .list_hosts()
            .unwrap()
            .into_iter()
            .filter(|host| host.space == space)
            .map(|host| host.host.name)
            .collect();
        names.sort();
        names
    }

    fn epoch(&self, team: Uuid) -> u64 {
        let space = self
            .spaces
            .team_space(&self.spaces.main_id(), team)
            .unwrap();
        space.store.team_state().unwrap().unwrap().epoch
    }
}

/// A team `owner` made, with `others` let in under the roles given.
fn team_of(fake: &Fake, owner: &Device, others: &[(&Device, TeamRole)]) -> Uuid {
    let team = owner
        .on(fake, |teams| teams.create("Ops", "Olive"))
        .unwrap()
        .id;
    for (device, role) in others {
        let invite = owner.on(fake, |teams| teams.invite(team, *role)).unwrap();
        device
            .on(fake, |teams| teams.join(&invite.code.unwrap(), "Somebody"))
            .unwrap();
        let seen = device.fingerprint(fake);
        owner
            .on(fake, |teams| teams.confirm(team, device.account, &seen))
            .unwrap();
        device.pass(fake);
    }
    team
}

fn code_of<T: std::fmt::Debug>(result: Result<T>) -> String {
    match result {
        Err(error) => error.code().unwrap_or("none").to_string(),
        Ok(value) => panic!("should have been refused: {value:?}"),
    }
}

// ── What has to hold ─────────────────────────────────────────────────────────

#[test]
fn a_server_without_teams_changes_nothing() {
    let fake = Fake::default();
    let device = Device::new();
    fake.state.lock().old = true;
    let pass = device.pass(&fake);
    assert!(!pass.status.supported && !pass.spaces_changed && !pass.records_changed);
    assert_eq!(device.spaces.all().len(), 1);
    assert!(matches!(
        device.on(&fake, |teams| teams.create("Ops", "Olive")),
        Err(TeamError::Unsupported)
    ));

    // Switched off by the operator: known, and what is here stays.
    fake.state.lock().old = false;
    let team = team_of(&fake, &device, &[]);
    fake.state.lock().disabled = true;
    let pass = device.pass(&fake);
    assert!(pass.status.supported && !pass.status.enabled && !pass.status.may_create);
    assert!(device.space(team).is_some());
}

#[test]
fn somebody_who_joins_waits_then_reads_and_writes_what_the_team_has() {
    let fake = Fake::default();
    let (olive, ben) = (Device::new(), Device::new());
    let made = olive
        .on(&fake, |teams| teams.create("  Ops\n", "Olive"))
        .unwrap();
    assert_eq!(made.name, "Ops");
    assert_eq!(made.role, TeamRole::Owner);
    assert_eq!(made.space, olive.space(made.id));
    let team = made.id;
    olive.add(team, "bastion").unwrap();
    olive.pass(&fake);

    let invite = olive
        .on(&fake, |teams| teams.invite(team, TeamRole::Editor))
        .unwrap();
    assert_eq!(
        olive.on(&fake, |teams| teams.invites(team)).unwrap().len(),
        1
    );
    let joined = ben
        .on(&fake, |teams| {
            teams.join(invite.code.as_deref().unwrap(), "Ben")
        })
        .unwrap();
    assert_eq!(joined.state, MemberState::Pending);
    assert_eq!(
        code_of(ben.on(&fake, |teams| teams.join("CODE-nope", "Ben"))),
        "invalid-invite"
    );

    // Waiting: listed, and nothing of the team on the device.
    let waiting = ben.pass(&fake);
    assert_eq!(waiting.status.teams[0].state, MemberState::Pending);
    assert!(waiting.status.teams[0].space.is_none() && ben.space(team).is_none());

    let members = olive.on(&fake, |teams| teams.members(team)).unwrap();
    let listed = members
        .iter()
        .find(|m| m.account_id == ben.account)
        .unwrap();
    assert!(!listed.me && listed.state == MemberState::Pending);
    assert_eq!(listed.fingerprint, ben.fingerprint(&fake));
    assert!(members.iter().any(|m| m.me && m.role == TeamRole::Owner));

    // The fingerprint may be typed as people type: case and spacing.
    let typed = listed.fingerprint.to_lowercase().replace(' ', "");
    olive
        .on(&fake, |teams| teams.confirm(team, ben.account, &typed))
        .unwrap();

    let pass = ben.pass(&fake);
    assert!(pass.spaces_changed && pass.records_changed);
    assert_eq!(pass.status.teams[0].space, ben.space(team));
    assert_eq!(ben.hosts(team), ["bastion"]);
    let space = ben.spaces.get(&ben.space(team).unwrap()).unwrap();
    assert_eq!(space.label.as_deref(), Some("Ops"));
    assert_eq!(space.parent, Some(ben.spaces.main_id()));

    ben.add(team, "db").unwrap();
    ben.pass(&fake);
    olive.pass(&fake);
    assert_eq!(olive.hosts(team), ["bastion", "db"]);

    // Nothing left to do is nothing done.
    let idle = olive.pass(&fake);
    assert!(!idle.spaces_changed && !idle.records_changed);

    // The server holds no name it can read.
    for envelope in fake.mailbox(team).records() {
        let blob = String::from_utf8_lossy(&envelope.blob).to_string();
        assert!(!blob.contains("bastion") && !blob.contains("192.0.2.10"));
        assert_eq!(envelope.vault_id, team);
    }
}

#[test]
fn another_device_of_the_account_opens_the_team_with_the_same_member_key() {
    let fake = Fake::default();
    let laptop = Device::new();
    let team = team_of(&fake, &laptop, &[]);
    laptop.add(team, "bastion").unwrap();
    laptop.pass(&fake);

    let phone = laptop.second();
    let pass = phone.pass(&fake);
    assert_eq!(pass.status.fingerprint, Some(laptop.fingerprint(&fake)));
    assert_eq!(phone.hosts(team), ["bastion"]);

    // A vault that is not the account's cannot open its member key.
    let mut stranger = Device::new();
    stranger.account = laptop.account;
    assert!(matches!(
        stranger.on(&fake, |teams| teams.pass()),
        Err(TeamError::Untrusted(_))
    ));
    assert!(stranger.space(team).is_none());
}

#[test]
fn the_key_goes_to_the_fingerprint_that_was_compared_or_to_nobody() {
    let fake = Fake::default();
    let (olive, ben, mallory) = (Device::new(), Device::new(), Device::new());
    let team = team_of(&fake, &olive, &[]);
    let invite = olive
        .on(&fake, |teams| teams.invite(team, TeamRole::Editor))
        .unwrap();
    ben.on(&fake, |teams| teams.join(&invite.code.unwrap(), "Ben"))
        .unwrap();
    let seen = ben.fingerprint(&fake);

    // Another fingerprint than the listed key's: nothing is wrapped.
    mallory.on(&fake, |teams| teams.join("x", "M")).ok();
    let other = mallory.fingerprint(&fake);
    assert_eq!(
        code_of(olive.on(&fake, |teams| teams.confirm(team, ben.account, &other))),
        "identity-changed"
    );
    assert!(fake.wrapped(team, ben.account).is_none());

    // The server lists a key of its own under Ben's name. Olive compared
    // Ben's fingerprint with Ben; the key is not wrapped for the other one.
    let genuine = fake.state.lock().identities[&ben.account].0.clone();
    let swapped = fake.state.lock().identities[&mallory.account].0.clone();
    fake.team(team, |team| team.members[1].public = swapped);
    assert_eq!(
        code_of(olive.on(&fake, |teams| teams.confirm(team, ben.account, &seen))),
        "identity-changed"
    );
    assert!(fake.wrapped(team, ben.account).is_none());

    fake.team(team, |team| team.members[1].public = genuine);
    olive
        .on(&fake, |teams| teams.confirm(team, ben.account, &seen))
        .unwrap();
    assert!(fake.wrapped(team, ben.account).is_some());
}

#[test]
fn a_reader_reads_and_takes_no_write_until_the_role_says_so() {
    let fake = Fake::default();
    let (olive, rita) = (Device::new(), Device::new());
    let team = team_of(&fake, &olive, &[(&rita, TeamRole::Reader)]);
    olive.add(team, "bastion").unwrap();
    olive.pass(&fake);
    rita.pass(&fake);
    assert_eq!(rita.hosts(team), ["bastion"]);
    assert!(
        rita.spaces
            .get(&rita.space(team).unwrap())
            .unwrap()
            .read_only
    );
    assert!(rita.add(team, "mine").is_err());
    let before = fake.mailbox(team).cursor();
    rita.pass(&fake);
    assert_eq!(fake.mailbox(team).cursor(), before);
    // What a reader may not do, the server says too.
    assert_eq!(
        code_of(rita.on(&fake, |teams| teams.invite(team, TeamRole::Editor))),
        "forbidden"
    );

    olive
        .on(&fake, |teams| {
            teams.set_role(team, rita.account, TeamRole::Editor)
        })
        .unwrap();
    assert!(rita.pass(&fake).spaces_changed);
    rita.add(team, "mine").unwrap();
    rita.pass(&fake);
    olive.pass(&fake);
    assert_eq!(olive.hosts(team), ["bastion", "mine"]);
}

#[test]
fn removing_a_member_gives_the_team_a_new_key_and_names_the_logins_they_knew() {
    let fake = Fake::default();
    let (olive, ben, cleo) = (Device::new(), Device::new(), Device::new());
    let team = team_of(
        &fake,
        &olive,
        &[(&ben, TeamRole::Editor), (&cleo, TeamRole::Editor)],
    );
    let secret = PasswordChange::Set {
        value: SecretText::new("hunter2"),
    };
    let bastion = olive.add_with(team, "bastion", secret).unwrap();
    olive.add(team, "db").unwrap();
    olive.pass(&fake);
    ben.pass(&fake);
    cleo.pass(&fake);
    // Cleo has an edit nobody has seen when the key changes.
    cleo.add(team, "cleo's").unwrap();
    let old_epoch = fake.epoch(team);
    let old_key = cleo
        .spaces
        .get(&cleo.space(team).unwrap())
        .unwrap()
        .store
        .team_key()
        .unwrap();

    let known = olive
        .on(&fake, |teams| teams.remove(team, ben.account))
        .unwrap();
    assert_eq!(known.len(), 1);
    assert_eq!((known[0].id, known[0].name.as_str()), (bastion, "bastion"));
    assert_eq!(fake.epoch(team), old_epoch + 1);
    assert_eq!(olive.epoch(team), old_epoch + 1);
    assert!(!fake.team(team, |team| team.needs_rotation));
    // Everything is on the server again, under the new key.
    assert_eq!(olive.hosts(team), ["bastion", "db"]);
    assert!(fake.mailbox(team).len() >= 2);

    // Ben is out: the team leaves his device.
    let pass = ben.pass(&fake);
    assert!(pass.spaces_changed && pass.status.teams.is_empty());
    assert!(ben.space(team).is_none());

    // Cleo follows, and her edit arrives under the new key.
    cleo.pass(&fake);
    assert_eq!(cleo.epoch(team), old_epoch + 1);
    let new_key = cleo
        .spaces
        .get(&cleo.space(team).unwrap())
        .unwrap()
        .store
        .team_key()
        .unwrap();
    assert_ne!(*old_key, *new_key);
    assert_eq!(cleo.hosts(team), ["bastion", "cleo's", "db"]);
    olive.pass(&fake);
    assert_eq!(olive.hosts(team), ["bastion", "cleo's", "db"]);
    assert_eq!(
        fake.state.lock().rotations,
        2,
        "the one at creation and this one"
    );

    // The password came along through the change of key.
    let space = cleo.spaces.get(&cleo.space(team).unwrap()).unwrap();
    let host = space.store.get_host(bastion).unwrap().unwrap();
    assert!(host.has_password);
}

#[test]
fn a_rotation_that_stopped_halfway_is_finished_on_the_next_pass() {
    let fake = Fake::default();
    let (olive, ben, cleo) = (Device::new(), Device::new(), Device::new());
    let team = team_of(
        &fake,
        &olive,
        &[(&ben, TeamRole::Editor), (&cleo, TeamRole::Editor)],
    );
    olive.add(team, "bastion").unwrap();
    olive.pass(&fake);
    cleo.pass(&fake);

    // The server switches and the answer never arrives: its records are
    // gone, and this device does not know whether it happened.
    fake.state.lock().drop_rotate_answer = true;
    olive
        .on(&fake, |teams| teams.remove(team, ben.account))
        .unwrap();
    assert_eq!(fake.epoch(team), 3);
    assert_eq!(olive.epoch(team), 2);
    assert!(fake.mailbox(team).is_empty());

    olive.pass(&fake);
    assert_eq!(olive.epoch(team), 3);
    assert!(!fake.mailbox(team).is_empty());
    cleo.pass(&fake);
    assert_eq!(cleo.hosts(team), ["bastion"]);
    assert_eq!(fake.state.lock().rotations, 2);

    // One that was refused leaves nothing behind either.
    fake.team(team, |team| team.needs_rotation = true);
    fake.state.lock().refuse_rotate = Some("stale-cursor");
    let pass = olive.pass(&fake);
    assert!(!pass.status.teams[0].needs_rotation);
    assert_eq!((fake.epoch(team), olive.epoch(team)), (4, 4));
    cleo.pass(&fake);
    assert_eq!(cleo.hosts(team), ["bastion"]);
}

#[test]
fn two_who_manage_a_team_end_with_one_rotation() {
    let fake = Fake::default();
    let (olive, adam, ben) = (Device::new(), Device::new(), Device::new());
    let team = team_of(
        &fake,
        &olive,
        &[(&adam, TeamRole::Admin), (&ben, TeamRole::Editor)],
    );
    olive.add(team, "bastion").unwrap();
    olive.pass(&fake);
    adam.pass(&fake);
    adam.add(team, "adam's").unwrap();

    // Ben leaves by himself; whoever comes by first rotates.
    ben.on(&fake, |teams| teams.leave(team)).unwrap();
    assert!(ben.space(team).is_none());
    assert!(fake.team(team, |team| team.needs_rotation));
    let before = fake.state.lock().rotations;
    adam.pass(&fake);
    olive.pass(&fake);
    adam.pass(&fake);
    assert_eq!(fake.state.lock().rotations, before + 1);
    assert_eq!((olive.epoch(team), adam.epoch(team)), (3, 3));
    assert_eq!(olive.hosts(team), ["adam's", "bastion"]);
    assert_eq!(adam.hosts(team), ["adam's", "bastion"]);

    // The last owner does not leave.
    assert_eq!(
        code_of(olive.on(&fake, |teams| teams.leave(team))),
        "last-owner"
    );
    assert!(olive.space(team).is_some());
    olive.pass(&fake);
    assert_eq!(olive.hosts(team), ["adam's", "bastion"]);
}

#[test]
fn a_member_the_server_lists_and_nobody_confirmed_gets_no_key() {
    let fake = Fake::default();
    let (olive, ben, mallory) = (Device::new(), Device::new(), Device::new());
    let team = team_of(&fake, &olive, &[(&ben, TeamRole::Editor)]);
    olive.add(team, "bastion").unwrap();
    olive.pass(&fake);

    // The server puts an account of its choosing among the active members
    // and asks for a new key.
    mallory.on(&fake, |teams| teams.join("x", "M")).ok();
    let public = fake.state.lock().identities[&mallory.account].0.clone();
    fake.team(team, |team| {
        team.members.push(Member {
            account: mallory.account,
            display: "Ben".into(),
            role: TeamRole::Editor,
            state: MemberState::Active,
            public,
            wrapped: None,
            epoch: None,
        });
        team.needs_rotation = true;
    });
    let epoch = fake.epoch(team);
    let pass = olive.pass(&fake);
    assert!(pass.status.teams[0].needs_rotation);
    assert_eq!(fake.epoch(team), epoch);
    assert!(fake.wrapped(team, mallory.account).is_none());

    // The same for a confirmed member whose key was swapped afterwards.
    fake.team(team, |team| {
        team.members.pop();
        let swapped = team.members[0].public.clone();
        team.members[1].public = swapped;
    });
    olive.pass(&fake);
    assert_eq!(fake.epoch(team), epoch);
}

#[test]
fn a_key_of_the_servers_own_or_an_old_one_again_is_not_taken() {
    let fake = Fake::default();
    let (olive, ben, cleo) = (Device::new(), Device::new(), Device::new());
    let team = team_of(
        &fake,
        &olive,
        &[(&ben, TeamRole::Editor), (&cleo, TeamRole::Editor)],
    );
    olive.add(team, "bastion").unwrap();
    olive.pass(&fake);
    cleo.pass(&fake);
    let epoch = cleo.epoch(team);
    let first_wrap = fake.wrapped(team, cleo.account).unwrap();

    // A wrap anybody can make: for Cleo's key, the next epoch, a key the
    // server knows. It does not carry the key Cleo holds.
    let public = fake.state.lock().identities[&cleo.account].0.clone();
    let public: [u8; 32] = decode_base64(&public).unwrap().try_into().unwrap();
    let context = WrapContext {
        team,
        epoch: epoch + 1,
        account: cleo.account,
    };
    let forged = wrap_team_key(&new_team_key(), &[], context, &public).unwrap();
    let genuine = fake.wrapped(team, cleo.account);
    fake.team(team, |team| {
        team.epoch += 1;
        team.members[2].wrapped = Some(encode_base64(forged));
    });
    cleo.add(team, "cleo's").unwrap();
    let cursor = fake.mailbox(team).cursor();
    cleo.pass(&fake);
    assert_eq!(cleo.epoch(team), epoch);
    assert_eq!(fake.mailbox(team).cursor(), cursor, "nothing sealed for it");
    assert_eq!(cleo.hosts(team), ["bastion", "cleo's"]);
    // Another key under the epoch held is no better.
    let context = WrapContext { epoch, ..context };
    let forged = wrap_team_key(&new_team_key(), &[], context, &public).unwrap();
    fake.team(team, |team| {
        team.epoch -= 1;
        team.members[2].wrapped = Some(encode_base64(forged));
    });
    cleo.pass(&fake);
    assert_eq!(fake.mailbox(team).cursor(), cursor);
    fake.team(team, |team| team.members[2].wrapped = genuine);

    // A real rotation, and then the wrap from before it again.
    olive
        .on(&fake, |teams| teams.remove(team, ben.account))
        .unwrap();
    cleo.pass(&fake);
    assert_eq!(cleo.epoch(team), epoch + 1);
    olive.pass(&fake);
    assert_eq!(olive.hosts(team), ["bastion", "cleo's"]);
    fake.team(team, |team| {
        team.members[1].wrapped = Some(first_wrap);
        team.members[1].epoch = Some(epoch);
    });
    cleo.add(team, "later").unwrap();
    let cursor = fake.mailbox(team).cursor();
    cleo.pass(&fake);
    assert_eq!(cleo.epoch(team), epoch + 1);
    assert_eq!(fake.mailbox(team).cursor(), cursor);
}

#[test]
fn a_team_that_is_gone_leaves_every_device() {
    let fake = Fake::default();
    let (olive, ben) = (Device::new(), Device::new());
    let team = team_of(&fake, &olive, &[(&ben, TeamRole::Editor)]);
    olive.add(team, "bastion").unwrap();
    olive.pass(&fake);
    ben.pass(&fake);
    assert_eq!(
        code_of(ben.on(&fake, |teams| teams.delete(team))),
        "forbidden"
    );

    olive
        .on(&fake, |teams| teams.rename(team, "Operations"))
        .unwrap();
    assert!(ben.pass(&fake).spaces_changed);
    let space = ben.spaces.get(&ben.space(team).unwrap()).unwrap();
    assert_eq!(space.label.as_deref(), Some("Operations"));

    // Not reachable is not gone.
    fake.state.lock().down = true;
    assert!(matches!(
        ben.on(&fake, |teams| teams.pass()),
        Err(TeamError::Offline(_))
    ));
    assert!(ben.space(team).is_some());
    fake.state.lock().down = false;

    olive.on(&fake, |teams| teams.delete(team)).unwrap();
    assert!(olive.space(team).is_none());
    assert!(ben.pass(&fake).spaces_changed);
    assert!(ben.space(team).is_none());
    assert_eq!(ben.spaces.all().len(), 1);
}

#[test]
fn a_locked_vault_asks_for_the_password_and_touches_nothing() {
    let fake = Fake::default();
    let olive = Device::new();
    let team = team_of(&fake, &olive, &[]);
    olive.spaces.lock_all();
    assert!(matches!(
        olive.on(&fake, |teams| teams.pass()),
        Err(TeamError::VaultLocked)
    ));
    assert!(matches!(
        olive.on(&fake, |teams| teams.create("Two", "Olive")),
        Err(TeamError::VaultLocked)
    ));
    assert!(olive.space(team).is_some());
}

#[test]
fn what_went_wrong_is_one_of_the_words_the_interface_knows() {
    let kind = |code: &str| refused::<()>(code).unwrap_err().kind();
    assert_eq!(kind("team-limit"), "limit");
    assert_eq!(kind("team-full"), "limit");
    assert_eq!(kind("too-many-invites"), "limit");
    assert_eq!(kind("invalid-invite"), "invalid-code");
    assert_eq!(kind("last-owner"), "last-owner");
    assert_eq!(kind("forbidden"), "not-allowed");
    assert_eq!(kind("team-creation-closed"), "not-allowed");
    assert_eq!(kind("membership-pending"), "not-allowed");
    assert_eq!(kind("teams-disabled"), "unsupported");
    assert_eq!(kind("something-new"), "error");
    assert_eq!(TeamError::Unsupported.kind(), "unsupported");
    assert_eq!(TeamError::VaultLocked.kind(), "vault-locked");
    assert_eq!(TeamError::Offline("x".into()).kind(), "offline");
    assert_eq!(TeamError::Untrusted("x").kind(), "error");
    // And the shapes the page reads.
    let info = serde_json::to_value(crate::TeamInfo {
        id: Uuid::nil(),
        name: "Ops".into(),
        role: TeamRole::Admin,
        state: MemberState::Pending,
        members: 2,
        needs_rotation: true,
        space: None,
    })
    .unwrap();
    assert_eq!(info["role"], "admin");
    assert_eq!(info["state"], "pending");
    assert_eq!(info["needsRotation"], true);
    assert!(info["space"].is_null());
    let status = serde_json::to_value(crate::ServerTeams::default()).unwrap();
    assert_eq!(status["mayCreate"], false);
    assert!(status["fingerprint"].is_null());
}
