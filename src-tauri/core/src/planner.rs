//! Route planner: find breeding routes from owned (or wild-catchable) pals to
//! a target species, optionally carrying a set of desired passives.
//!
//! Both modes from PLAN.md are one search: states are (species, covered
//! passives, steps) kept as a pareto front per species — with no desired
//! passives this degenerates to plain shortest-route search. Passive coverage
//! is the deterministic "parent pool" model (see Mechanics Confidence): a bred
//! child can draw from the union of both parents' passives; actual inheritance
//! odds are community-tested only, so we never pretend to compute probability.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::data::GameData;
use crate::types::*;

// Generous default: the search converges (frontier drains) long before this
// in practice, so a big budget mostly buys completeness, not runtime.
const DEFAULT_MAX_STEPS: u32 = 500;
const DEFAULT_MAX_ROUTES: usize = 10;
const MAX_DESIRED: usize = 12;
const PARETO_CAP: usize = 24;

#[derive(Debug, Clone, Deserialize)]
pub struct OwnedPal {
    pub species: TribeKey,
    #[serde(default)]
    pub label: String,
    /// Internal passive keys (from passive_skills_assignable.json)
    #[serde(default)]
    pub passives: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanRequest {
    pub target: TribeKey,
    #[serde(default)]
    pub desired_passives: Vec<String>,
    #[serde(default)]
    pub owned: Vec<OwnedPal>,
    /// Treat every obtainable species (has an icon) as a free 0-step leaf.
    #[serde(default)]
    pub assume_wild: bool,
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub max_routes: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteNode {
    pub species: TribeKey,
    pub name: String,
    pub icon: Option<String>,
    /// Set for leaves: the owned pal's label, or "wild" for assumed catches.
    pub owned: Option<String>,
    /// Display names of desired passives this leaf contributes (leaves only).
    pub passives: Vec<String>,
    /// Gender requirements for parents[0] / parents[1] (gendered specials).
    pub gender_a: Option<Gender>,
    pub gender_b: Option<Gender>,
    /// Empty for leaves, exactly [parent_a, parent_b] for bred nodes.
    pub parents: Vec<RouteNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Route {
    pub steps: u32,
    /// Display names of desired passives covered / not covered by this route.
    pub covered: Vec<String>,
    pub missing: Vec<String>,
    pub root: RouteNode,
}

/// Search diagnostics so the UI can explain fast returns: once the pareto
/// fronts saturate the frontier empties and deeper step budgets change nothing.
#[derive(Debug, Clone, Serialize)]
pub struct PlanStats {
    /// The step budget this plan actually ran with (echoed so the UI reports
    /// the used budget, not whatever the input field says now).
    pub max_steps: u32,
    /// Rounds actually executed (each round can add one breeding step of depth).
    pub rounds: u32,
    /// True when the search exhausted itself before the step budget — a higher
    /// max_steps cannot produce different results for this input.
    pub converged: bool,
    /// Non-dominated states explored across all species.
    pub states: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanOutcome {
    pub routes: Vec<Route>,
    pub stats: PlanStats,
}

#[derive(Debug, Clone, Copy)]
enum Prov {
    Owned(u32),
    Wild,
    Bred {
        a: u32,
        b: u32,
        gender_a: Option<Gender>,
        gender_b: Option<Gender>,
    },
}

#[derive(Debug, Clone, Copy)]
struct St {
    species: u16,
    mask: u16,
    steps: u32,
    prov: Prov,
}

pub fn plan_routes(gd: &GameData, req: &PlanRequest) -> Result<PlanOutcome, String> {
    let started = std::time::Instant::now();
    if !gd.pals.contains_key(&req.target) {
        return Err(format!("unknown target species: {}", req.target));
    }
    let desired: Vec<String> = req
        .desired_passives
        .iter()
        .take(MAX_DESIRED)
        .cloned()
        .collect();
    for p in &desired {
        if !gd.passives.contains_key(p) {
            return Err(format!("unknown passive: {p}"));
        }
    }
    // No upper cap: the fixpoint below stops as soon as a round finds nothing
    // new, so huge values converge — they just search longer first.
    let max_steps = req.max_steps.unwrap_or(DEFAULT_MAX_STEPS).max(1);
    let max_routes = req.max_routes.unwrap_or(DEFAULT_MAX_ROUTES).clamp(1, 50);

    // Interned species + full combo table, built once per GameData and cached.
    let table = gd.pair_table();
    let keys = &table.keys;
    let idx = &table.idx;
    let pair_children = &table.children;
    let n = keys.len();

    let passive_bit: HashMap<&str, u16> = desired
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), 1u16 << i))
        .collect();

    let mut states: Vec<St> = Vec::new();
    let mut per_species: Vec<Vec<u32>> = vec![Vec::new(); n];

    // Seed owned pals first so they dominate equivalent wild seeds.
    let mut frontier: Vec<u32> = Vec::new();
    for (oi, o) in req.owned.iter().enumerate() {
        let Some(&sp) = idx.get(o.species.as_str()) else {
            return Err(format!("unknown owned species: {}", o.species));
        };
        let mask = o
            .passives
            .iter()
            .filter_map(|p| passive_bit.get(p.as_str()))
            .fold(0u16, |acc, b| acc | b);
        if let Some(id) = insert(
            &mut states,
            &mut per_species,
            St {
                species: sp,
                mask,
                steps: 0,
                prov: Prov::Owned(oi as u32),
            },
        ) {
            frontier.push(id);
        }
    }
    if req.assume_wild {
        for (i, key) in keys.iter().enumerate() {
            if !gd.icons.contains_key(key) {
                continue; // icon-less tribes are unobtainable boss/unreleased pals
            }
            if let Some(id) = insert(
                &mut states,
                &mut per_species,
                St {
                    species: i as u16,
                    mask: 0,
                    steps: 0,
                    prov: Prov::Wild,
                },
            ) {
                frontier.push(id);
            }
        }
    }
    if states.is_empty() {
        return Err("no owned pals given (and wild catches not assumed)".into());
    }

    // Semi-naive fixpoint: each round combines newly created states with all
    // known states. Converges when a round produces nothing new. Every state
    // born in round r has steps >= r (it uses a round r-1 state), so max_steps
    // rounds are enough to discover everything within the step budget.
    let mut rounds = 0u32;
    let mut budget_clipped = false;
    for _ in 0..max_steps {
        if frontier.is_empty() {
            break;
        }
        rounds += 1;
        let all_ids: Vec<u32> = per_species.iter().flatten().copied().collect();
        let mut newly: Vec<u32> = Vec::new();
        for &f in &frontier {
            for &s in &all_ids {
                if s == f {
                    continue; // one pal instance can't breed with itself
                }
                let (sa, sb) = (states[f as usize], states[s as usize]);
                let steps = sa.steps + sb.steps + 1;
                let mask = sa.mask | sb.mask;
                let swapped = sa.species > sb.species;
                let key = if swapped {
                    (sb.species, sa.species)
                } else {
                    (sa.species, sb.species)
                };
                if steps > max_steps {
                    // The budget clipped this candidate. If it could have been
                    // novel for any child species (not already dominated), a
                    // higher budget could change results — exhaustion must not
                    // be claimed. An emptied frontier only means "explored
                    // everything within the budget".
                    if !budget_clipped {
                        budget_clipped = pair_children[&key].iter().any(|&(child, _, _)| {
                            !dominated(&states, &per_species[child as usize], mask, steps)
                        });
                    }
                    continue;
                }
                for &(child, ga, gb) in &pair_children[&key] {
                    let (ga, gb) = if swapped { (gb, ga) } else { (ga, gb) };
                    if let Some(id) = insert(
                        &mut states,
                        &mut per_species,
                        St {
                            species: child,
                            mask,
                            steps,
                            prov: Prov::Bred {
                                a: f,
                                b: s,
                                gender_a: ga,
                                gender_b: gb,
                            },
                        },
                    ) {
                        newly.push(id);
                    }
                }
            }
        }
        // Pareto pruning may have evicted some of the new states already.
        let live: std::collections::HashSet<u32> =
            per_species.iter().flatten().copied().collect();
        frontier = newly.into_iter().filter(|id| live.contains(id)).collect();
    }

    let converged = frontier.is_empty() && !budget_clipped;

    // Rank target states by (coverage desc, steps asc) and build route trees.
    let target_idx = idx[req.target.as_str()];
    let mut candidates: Vec<u32> = per_species[target_idx as usize].clone();
    candidates.sort_by_key(|&id| {
        let s = &states[id as usize];
        (std::cmp::Reverse(s.mask.count_ones()), s.steps)
    });

    let desired_names: Vec<String> = desired
        .iter()
        .map(|p| gd.passives[p].name.clone())
        .collect();

    let routes: Vec<Route> = candidates
        .into_iter()
        .take(max_routes)
        .map(|id| {
            let s = &states[id as usize];
            let covered: Vec<String> = (0..desired.len())
                .filter(|i| s.mask & (1 << i) != 0)
                .map(|i| desired_names[i].clone())
                .collect();
            let missing: Vec<String> = (0..desired.len())
                .filter(|i| s.mask & (1 << i) == 0)
                .map(|i| desired_names[i].clone())
                .collect();
            Route {
                steps: s.steps,
                covered,
                missing,
                root: build_node(gd, &keys, &states, &req.owned, &passive_bit, id),
            }
        })
        .collect();

    Ok(PlanOutcome {
        routes,
        stats: PlanStats {
            max_steps,
            rounds,
            converged,
            states: states.len(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        },
    })
}

/// True when some existing state covers a superset of `mask` in no more steps.
fn dominated(states: &[St], list: &[u32], mask: u16, steps: u32) -> bool {
    list.iter().any(|&id| {
        let e = &states[id as usize];
        e.mask & mask == mask && e.steps <= steps
    })
}

/// Pareto insert: reject if dominated; evict states the newcomer dominates.
fn insert(states: &mut Vec<St>, per_species: &mut [Vec<u32>], st: St) -> Option<u32> {
    if dominated(states, &per_species[st.species as usize], st.mask, st.steps) {
        return None;
    }
    let list = &mut per_species[st.species as usize];
    list.retain(|&id| {
        let e = &states[id as usize];
        !(st.mask & e.mask == e.mask && st.steps <= e.steps)
    });
    if list.len() >= PARETO_CAP {
        // Evict the weakest entry if the newcomer beats it.
        let (pos, &worst) = list
            .iter()
            .enumerate()
            .min_by_key(|(_, &id)| {
                let e = &states[id as usize];
                (e.mask.count_ones(), std::cmp::Reverse(e.steps))
            })
            .expect("cap > 0");
        let w = &states[worst as usize];
        if (st.mask.count_ones(), std::cmp::Reverse(st.steps))
            <= (w.mask.count_ones(), std::cmp::Reverse(w.steps))
        {
            return None;
        }
        list.swap_remove(pos);
    }
    let id = states.len() as u32;
    states.push(st);
    per_species[st.species as usize].push(id);
    Some(id)
}

fn build_node(
    gd: &GameData,
    keys: &[TribeKey],
    states: &[St],
    owned: &[OwnedPal],
    passive_bit: &HashMap<&str, u16>,
    id: u32,
) -> RouteNode {
    let s = &states[id as usize];
    let species = keys[s.species as usize].clone();
    let info = &gd.pals[&species];
    let mut node = RouteNode {
        name: info.name.clone(),
        icon: gd.icons.get(&species).cloned(),
        species,
        owned: None,
        passives: Vec::new(),
        gender_a: None,
        gender_b: None,
        parents: Vec::new(),
    };
    match s.prov {
        Prov::Wild => node.owned = Some("wild".into()),
        Prov::Owned(oi) => {
            let o = &owned[oi as usize];
            node.owned = Some(if o.label.is_empty() {
                info.name.clone()
            } else {
                o.label.clone()
            });
            node.passives = o
                .passives
                .iter()
                .filter(|p| passive_bit.contains_key(p.as_str()))
                .map(|p| gd.passives[p].name.clone())
                .collect();
        }
        Prov::Bred {
            a,
            b,
            gender_a,
            gender_b,
        } => {
            node.gender_a = gender_a;
            node.gender_b = gender_b;
            node.parents = vec![
                build_node(gd, keys, states, owned, passive_bit, a),
                build_node(gd, keys, states, owned, passive_bit, b),
            ];
        }
    }
    node
}
