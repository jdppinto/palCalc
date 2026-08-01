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
