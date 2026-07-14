use palcalc_core::{GameData, Gender};

fn data() -> GameData {
    GameData::load().expect("embedded data must parse")
}

#[test]
fn registry_counts_match_verified_data() {
    let d = data();
    assert_eq!(d.pals.len(), 333, "breeding_data.json tribes");
    assert_eq!(
        d.pals.values().filter(|p| p.child_eligible).count(),
        256,
        "child-eligible pals"
    );
    assert_eq!(d.passives.len(), 114, "assignable passives");
    assert_eq!(d.icons.len(), 426, "icon_map entries");
}

#[test]
fn placeholder_names_are_fixed_for_ui_visible_pals() {
    let d = data();
    assert_eq!(d.pals["Blueplatypus"].name, "Fuack");
    assert_eq!(d.pals["WindChimes"].name, "Hangyu");
    assert_eq!(d.pals["WindChimes_Ice"].name, "Hangyu Cryst");

    // Guard against a data refresh reintroducing placeholder names in pals the
    // UI can show (icon-bearing). These pals legitimately share codename and
    // display name (verified: Anubis, Suzaku, Sekhmet #140).
    let legit = ["Anubis", "Suzaku", "Sekhmet"];
    let placeholders: Vec<_> = d
        .pals
        .values()
        .filter(|p| {
            d.icons.contains_key(&p.key)
                && p.name.eq_ignore_ascii_case(&p.key.replace('_', ""))
                && !legit.contains(&p.key.as_str())
        })
        .map(|p| p.key.clone())
        .collect();
    assert!(
        placeholders.is_empty(),
        "UI-visible pals with placeholder names — add to data/name_fixes.json: {placeholders:?}"
    );
}

#[test]
fn icon_less_tribes_are_all_non_breedable() {
    let d = data();
    let bad: Vec<_> = d
        .pals
        .values()
        .filter(|p| !d.icons.contains_key(&p.key) && p.child_eligible)
        .map(|p| p.key.clone())
        .collect();
    assert!(bad.is_empty(), "eligible pals without icons: {bad:?}");
}

#[test]
fn same_species_breeds_itself() {
    let d = data();
    let out = d.breed("SheepBall", "SheepBall").unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].child, "SheepBall");
    // Also for a child_eligible=false variant (only reproduces same-species)
    let out = d.breed("LazyDragon_Electric", "LazyDragon_Electric").unwrap();
    assert_eq!(out[0].child, "LazyDragon_Electric");
}

#[test]
fn special_combo_overrides_rank_formula() {
    let d = data();
    let out = d.breed("LazyDragon", "ElecCat").unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].child, "LazyDragon_Electric");
    assert_eq!(out[0].gender_a, None);
    // Order-independent
    assert_eq!(d.breed("ElecCat", "LazyDragon").unwrap()[0].child, "LazyDragon_Electric");
}

#[test]
fn gendered_combos_swap_with_argument_order() {
    let d = data();
    // Katress (CatMage) x Wixen (FoxMage): the only 2 gendered combos in the game
    let out = d.breed("CatMage", "FoxMage").unwrap();
    assert_eq!(out.len(), 2);
    let dark = out.iter().find(|o| o.child == "FoxMage_Dark").unwrap();
    assert_eq!((dark.gender_a, dark.gender_b), (Some(Gender::Male), Some(Gender::Female)));
    let fire = out.iter().find(|o| o.child == "CatMage_Fire").unwrap();
    assert_eq!((fire.gender_a, fire.gender_b), (Some(Gender::Female), Some(Gender::Male)));

    // Swapped call order must swap the gender requirements
    let out = d.breed("FoxMage", "CatMage").unwrap();
    let dark = out.iter().find(|o| o.child == "FoxMage_Dark").unwrap();
    assert_eq!((dark.gender_a, dark.gender_b), (Some(Gender::Female), Some(Gender::Male)));
}

#[test]
fn rank_formula_regression_lamball_cattiva() {
    // Verified against current ranks during planning: Lamball + Cattiva → Tanzee
    // (the legacy launch-era table said Lamball — stale).
    let d = data();
    let out = d.breed("SheepBall", "PinkCat").unwrap();
    assert_eq!(d.pals[&out[0].child].name, "Tanzee");
}

#[test]
fn rank_formula_is_argmin_over_eligible_pool() {
    let d = data();
    let mut keys: Vec<_> = d.pals.keys().cloned().collect();
    keys.sort();
    // Sampled exhaustive check: every 7th pal paired with every 11th
    let sample_a: Vec<_> = keys.iter().step_by(7).collect();
    let sample_b: Vec<_> = keys.iter().step_by(11).collect();
    for a in &sample_a {
        for b in &sample_b {
            if a == b || d.is_special_pair(a, b) {
                continue;
            }
            let out = d.breed(a, b).unwrap();
            assert_eq!(out.len(), 1);
            let child = &out[0].child;
            let target = (d.pals[*a].rank + d.pals[*b].rank + 1) / 2;
            let child_dist = (d.pals[child].rank - target).abs();
            let min_dist = d
                .pals
                .values()
                .filter(|p| p.child_eligible)
                .map(|p| (p.rank - target).abs())
                .min()
                .unwrap();
            assert!(d.pals[child].child_eligible, "{a} + {b} -> ineligible {child}");
            assert_eq!(
                child_dist, min_dist,
                "{a} + {b} -> {child}: dist {child_dist} != min {min_dist}"
            );
        }
    }
}

#[test]
fn all_icons_exist_and_are_square() {
    // The plan originally claimed all icons are 128x128. Verified reality:
    // 423 are 128x128 and SnowTigerBeastman.png is 512x512, so the scanner
    // must normalize template sizes at load instead of trusting file dims.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/icons");
    let d = data();
    for file in d.icons.values() {
        let path = dir.join(file);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(bytes.len() > 24 && bytes[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        assert_eq!(w, h, "{file} is not square ({w}x{h})");
        assert!(w == 128 || w == 512, "{file} has unexpected size {w}x{h}");
    }
}
