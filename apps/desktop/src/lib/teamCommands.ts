/**
 * Teams, the page's side: the commands, and the two things several pages
 * watch – which servers have teams, and which spaces are read-only.
 *
 * `server` is the id of the server space a team lives on, `team` the team's
 * id. A command that fails rejects with a `TeamFailure` (see `asTeamFailure`).
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useState } from 'react';
import { spacesStatus, type SpacesStatus } from './sync';
import type { TeamInfo, TeamInvite, TeamMember, TeamRemoval, TeamRole, TeamServer } from './teams';

/** What the last sync pass learned; never waits for the network. */
export const teamsStatus = () => invoke<TeamServer[]>('teams_status');

/** `displayName` is how the other members see this account. */
export const teamCreate = (server: string, name: string, displayName: string) =>
  invoke<TeamInfo>('team_create', { server, name, displayName });
export const teamRename = (server: string, team: string, name: string) =>
  invoke<void>('team_rename', { server, team, name });
export const teamDelete = (server: string, team: string) =>
  invoke<void>('team_delete', { server, team });
export const teamLeave = (server: string, team: string) =>
  invoke<void>('team_leave', { server, team });

/** The answer carries the code, once. */
export const teamInvite = (server: string, team: string, role: TeamRole) =>
  invoke<TeamInvite>('team_invite', { server, team, role });
export const teamInvites = (server: string, team: string) =>
  invoke<TeamInvite[]>('team_invites', { server, team });
export const teamRevokeInvite = (server: string, team: string, invite: string) =>
  invoke<void>('team_revoke_invite', { server, team, invite });
/** Redeems a code: the team comes back `pending` until an admin confirms. */
export const teamJoin = (server: string, code: string, displayName: string) =>
  invoke<TeamInfo>('team_join', { server, code, displayName });

export const teamMembers = (server: string, team: string) =>
  invoke<TeamMember[]>('team_members', { server, team });
/**
 * Lets a waiting member in. `fingerprint` is the one the admin was shown and
 * compared: the backend hands the team key to exactly that key, or fails.
 */
export const teamConfirm = (server: string, team: string, account: string, fingerprint: string) =>
  invoke<void>('team_confirm', { server, team, account, fingerprint });
export const teamSetRole = (server: string, team: string, account: string, role: TeamRole) =>
  invoke<void>('team_set_role', { server, team, account, role });
/** Removes a member; the team key is replaced on its own afterwards. */
export const teamRemove = (server: string, team: string, account: string) =>
  invoke<TeamRemoval>('team_remove', { server, team, account });

/** Calls `refresh` now and whenever one of the events fires. */
function useEvents(events: readonly string[], refresh: () => void) {
  useEffect(() => {
    refresh();
    const stops = events.map((event) => listen(event, refresh));
    return () => {
      for (const stop of stops) void stop.then((unlisten) => unlisten()).catch(() => undefined);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh]);
}

const TEAM_EVENTS = ['teams:changed', 'spaces:changed'] as const;

/**
 * The servers and their teams, kept up to date. Null until the first answer,
 * and for good on a backend without teams.
 */
export function useTeamServers(): [TeamServer[] | null, () => void] {
  const [servers, setServers] = useState<TeamServer[] | null>(null);
  const refresh = useCallback(() => {
    void teamsStatus()
      .then(setServers)
      .catch(() => undefined);
  }, []);
  useEvents(TEAM_EVENTS, refresh);
  return [servers, refresh];
}

/**
 * `spaces_status`, kept up to date: a role that changes turns a team
 * read-only (or back) without the space coming or going.
 */
export function useSpaces(): SpacesStatus | null {
  const [spaces, setSpaces] = useState<SpacesStatus | null>(null);
  const refresh = useCallback(() => {
    void spacesStatus()
      .then(setSpaces)
      .catch(() => undefined);
  }, []);
  useEvents(TEAM_EVENTS, refresh);
  return spaces;
}
