use palcalc_core::{plan_routes, GameData, Gender, OwnedPal, PlanRequest, Route};

fn data() -> GameData {
    GameData::load().expect("embedded data must parse")
}

fn owned(species: &str, passives: &[&str]) -> OwnedPal {
    OwnedPal {
        species: species.into(),
        label: String::new(),
        passives: passives.iter().map(|s| s.to_string()).collect(),
    }
}

fn req(target: &str) -> PlanRequest {
    PlanRequest {
        target: target.into(),
        desired_passives: vec![],
        owned: vec![],
        assume_wild: false,
        max_steps: None,
        max_routes: None,
    }
}

fn count_bred_nodes(node: &palcalc_core::RouteNode) -> u32 {
    if node.parents.is_empty() {
        0
    } else {
        1 + node.parents.iter().map(count_bred_nodes).sum::<u32>()
    }
}

#[test]
fn one_step_route_from_owned_parents() {
    let d = data();
    // Verified in breeding tests: Lamball (SheepBall) + Cattiva (PinkCat) → Tanzee
    let tanzee = d.breed("SheepBall", "PinkCat").unwrap()[0].child.clone();
    let mut r = req(&tanzee);
    r.owned = vec![owned("SheepBall", &[]), owned("PinkCat", &[])];
    let routes = plan_routes(&d, &r).unwrap().routes;
    assert!(!routes.is_empty());
    assert_eq!(routes[0].steps, 1);
    assert_eq!(routes[0].root.species, tanzee);
    assert_eq!(routes[0].root.parents.len(), 2);
}

#[test]
fn zero_step_route_when_target_owned() {
    let d = data();
    let mut r = req("SheepBall");
    r.owned = vec![owned("SheepBall", &[])];
    let routes = plan_routes(&d, &r).unwrap().routes;
    assert_eq!(routes[0].steps, 0);
    assert!(routes[0].root.owned.is_some());
}

#[test]
fn wild_mode_reaches_special_combo_child_in_one_step() {
    let d = data();
    let mut r = req("LazyDragon_Electric");
    r.assume_wild = true;
    let routes = plan_routes(&d, &r).unwrap().routes;
    // LazyDragon_Electric itself has an icon (catchable), so 0 steps wins;
    // without it, the special combo would give 1.
    assert!(routes[0].steps <= 1);
}

#[test]
fn gendered_special_surfaces_requirements() {
    let d = data();
    let mut r = req("CatMage_Fire");
    r.owned = vec![owned("CatMage", &[]), owned("FoxMage", &[])];
    let routes = plan_routes(&d, &r).unwrap().routes;
    let root = &routes[0].root;
    assert_eq!(routes[0].steps, 1);
    // Katress ♀ × Wixen ♂ → Katress Ignis
    let genders = [root.gender_a, root.gender_b];
    assert!(genders.contains(&Some(Gender::Female)) && genders.contains(&Some(Gender::Male)));
}

#[test]
fn two_step_route_is_found() {
    let d = data();
    let step1 = d.breed("SheepBall", "PinkCat").unwrap()[0].child.clone();
    let step2 = d.breed(&step1, "SheepBall").unwrap()[0].child.clone();
    assert_ne!(step2, step1, "test premise: second cross must move species");
    let mut r = req(&step2);
    r.owned = vec![owned("SheepBall", &[]), owned("PinkCat", &[])];
    let routes = plan_routes(&d, &r).unwrap().routes;
    assert!(!routes.is_empty(), "no route found to {step2}");
    assert_eq!(routes[0].steps, 2);
    assert_eq!(count_bred_nodes(&routes[0].root), 2);
}

#[test]
fn passive_coverage_prefers_route_carrying_desired_passives() {
    let d = data();
    let tanzee = d.breed("SheepBall", "PinkCat").unwrap()[0].child.clone();
    // Two desired passives split across two owned pals: the route must use both.
    let (p1, p2) = ("Rare", "Legend");
    assert!(d.passives.contains_key(p1) && d.passives.contains_key(p2));
    let mut r = req(&tanzee);
    r.desired_passives = vec![p1.into(), p2.into()];
    r.owned = vec![owned("SheepBall", &[p1]), owned("PinkCat", &[p2])];
    let routes = plan_routes(&d, &r).unwrap().routes;
    let best: &Route = &routes[0];
    assert_eq!(best.covered.len(), 2, "both passives coverable: {best:?}");
    assert!(best.missing.is_empty());
    // Both leaves must be the owned carriers
    let leaves: Vec<_> = best.root.parents.iter().filter(|n| n.owned.is_some()).collect();
    assert_eq!(leaves.len(), 2);
}

#[test]
fn same_species_consolidation_merges_passives() {
    let d = data();
    // Two Lamballs each carrying one desired passive → breed them together to
    // get a Lamball child with both in its parent pool.
    let (p1, p2) = ("Rare", "Legend");
    let mut r = req("SheepBall");
    r.desired_passives = vec![p1.into(), p2.into()];
    r.owned = vec![owned("SheepBall", &[p1]), owned("SheepBall", &[p2])];
    let routes = plan_routes(&d, &r).unwrap().routes;
    let best = &routes[0];
    assert_eq!(best.covered.len(), 2);
    assert_eq!(best.steps, 1);
    assert_eq!(best.root.species, "SheepBall");
}

#[test]
fn deep_step_budget_converges_early_and_says_so() {
    let d = data();
    // The "40 steps is instant" case: pareto domination exhausts the frontier
    // after a few rounds, so a huge budget must converge and report it.
    let mut r = req("Anubis");
    r.assume_wild = true;
    r.max_steps = Some(40);
    let out = plan_routes(&d, &r).unwrap();
    assert_eq!(out.stats.max_steps, 40, "stats echo the budget actually used");
    assert!(out.stats.converged, "search should exhaust itself");
    assert!(
        out.stats.rounds < 40,
        "convergence should happen well before the budget, got {} rounds",
        out.stats.rounds
    );
    assert!(!out.routes.is_empty());
}

#[test]
fn convergence_claim_implies_budget_independence() {
    // Regression: the UI told the user "a higher step budget can't change
    // these results" while a higher budget absolutely changed them. If a
    // small-budget run claims convergence, a big-budget run must produce
    // identical results for every target.
    let d = data();
    let mut targets: Vec<_> = d
        .pals
        .keys()
        .filter(|k| d.icons.contains_key(*k))
        .cloned()
        .collect();
    targets.sort();
    for target in targets.iter().step_by(7) {
        let mut r = req(target);
        r.owned = vec![owned("SheepBall", &[]), owned("PinkCat", &["Rare"])];
        r.desired_passives = vec!["Rare".into()];
        r.max_steps = Some(3);
        let small = plan_routes(&d, &r).unwrap();
        r.max_steps = Some(12);
        let big = plan_routes(&d, &r).unwrap();

        let summary = |o: &palcalc_core::PlanOutcome| {
            o.routes
                .iter()
                .map(|r| (r.steps, r.covered.len()))
                .collect::<Vec<_>>()
        };
        if small.stats.converged {
            assert_eq!(
                summary(&small),
                summary(&big),
                "{target}: budget-3 claimed exhaustion but budget-12 differs"
            );
        }
    }
}

#[test]
fn unknown_inputs_are_rejected() {
    let d = data();
    assert!(plan_routes(&d, &req("NotAPal")).is_err());
    let mut r = req("SheepBall");
    r.owned = vec![owned("AlsoNotAPal", &[])];
    assert!(plan_routes(&d, &r).is_err());
    let mut r = req("SheepBall");
    r.owned = vec![owned("SheepBall", &[])];
    r.desired_passives = vec!["NotAPassive".into()];
    assert!(plan_routes(&d, &r).is_err());
}

#[test]
fn wild_mode_finds_routes_to_every_obtainable_species() {
    let d = data();
    // Every species with an icon should be reachable in wild mode (trivially,
    // by catching it). Sanity check the planner never errors across the roster.
    let mut checked = 0;
    for key in d.pals.keys() {
        if !d.icons.contains_key(key) {
            continue;
        }
        let mut r = req(key);
        r.assume_wild = true;
        r.max_steps = Some(2);
        let routes = plan_routes(&d, &r).unwrap().routes;
        assert!(!routes.is_empty(), "{key} unreachable in wild mode");
        assert_eq!(routes[0].steps, 0, "{key} should be catchable directly");
        checked += 1;
    }
    assert!(checked > 250, "expected ~299 obtainable species, got {checked}");
}
