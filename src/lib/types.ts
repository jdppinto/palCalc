export interface PalEntry {
  key: string;
  name: string;
  rank: number;
  child_eligible: boolean;
  icon: string | null;
}

export type Gender = "Male" | "Female";

export interface BreedResult {
  child: string;
  name: string;
  icon: string | null;
  gender_a: Gender | null;
  gender_b: Gender | null;
}

export interface PassiveEntry {
  key: string;
  name: string;
  rank: number;
}

export interface OwnedPal {
  species: string;
  label: string;
  passives: string[];
  gender: Gender | null;
}

export interface PlanRequest {
  target: string;
  desired_passives: string[];
  owned: OwnedPal[];
  assume_wild: boolean;
  max_steps?: number;
  max_routes?: number;
  reversers: number;
}

export interface RouteNode {
  species: string;
  name: string;
  icon: string | null;
  owned: string | null;
  passives: string[];
  all_passives: string[];
  covered_passives: string[];
  gender: Gender | null;
  gender_a: Gender | null;
  gender_b: Gender | null;
  parents: RouteNode[];
}

export interface Route {
  steps: number;
  covered: string[];
  missing: string[];
  root: RouteNode;
  reversers_used: number;
}

/// A saved breeding-tree result, persisted so a route can be recalled without
/// recomputing. `route` is the exact Route that was displayed.
export interface Bookmark {
  id: string;
  /// Stable structural identity of the route (see routeKey in bookmarks store),
  /// used for dedup instead of the human-readable label.
  key: string;
  label: string;
  saved_at: number;
  route: Route;
}

export interface PlanStats {
  max_steps: number;
  rounds: number;
  converged: boolean;
  states: number;
  elapsed_ms: number;
}

export interface PlanOutcome {
  routes: Route[];
  stats: PlanStats;
}

// --- Server mode (palcalc-server) ---

export interface ServerPlayer {
  uid: string;
  name: string;
  guild: string | null;
}

export type PalLocation = "palbox" | "party" | "base" | "unknown";

export interface ServerPal {
  species: string;
  gender: string; // "Male" | "Female" | "" (raw from the save)
  level: number;
  passives: string[];
  owner: string | null;
  location: PalLocation;
  container: string | null;
  guild: string | null;
}

export interface ServerRoster {
  generated_at_unix: number;
  players: ServerPlayer[];
  pals: ServerPal[];
}
