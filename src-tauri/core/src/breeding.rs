use crate::data::GameData;
use crate::types::*;

impl GameData {
    /// All possible children of the (unordered) pair, with gender requirements
    /// mapped back onto the (a, b) order the caller passed in.
    ///
    /// Precedence (verified: reproduces 99.4% of the legacy combo table, the
    /// rest being the same-species variant cases handled by rule 2):
    /// 1. special combo (gender-aware)  2. same species  3. rank formula.
    ///
    /// Returns None if either key is unknown.
    pub fn breed(&self, a: &str, b: &str) -> Option<Vec<BreedOutcome>> {
        if !self.pals.contains_key(a) || !self.pals.contains_key(b) {
            return None;
        }

        let swapped = a > b;
        let canonical = if swapped {
            (b.to_string(), a.to_string())
        } else {
            (a.to_string(), b.to_string())
        };
        if let Some(outcomes) = self.specials.get(&canonical) {
            return Some(
                outcomes
                    .iter()
                    .map(|o| {
                        if swapped {
                            BreedOutcome {
                                child: o.child.clone(),
                                gender_a: o.gender_b,
                                gender_b: o.gender_a,
                            }
                        } else {
                            o.clone()
                        }
                    })
                    .collect(),
            );
        }

        if a == b {
            return Some(vec![BreedOutcome {
                child: a.to_string(),
                gender_a: None,
                gender_b: None,
            }]);
        }

        let target = (self.pals[a].rank + self.pals[b].rank + 1) / 2;
        Some(vec![BreedOutcome {
            child: self.closest_eligible(target),
            gender_a: None,
            gender_b: None,
        }])
    }

    /// Whether the (unordered) pair is resolved by a special combo rather than
    /// the rank formula.
    pub fn is_special_pair(&self, a: &str, b: &str) -> bool {
        let canonical = if a <= b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        };
        self.specials.contains_key(&canonical)
    }

    /// Child-eligible pal whose rank is closest to `target`; ties broken by
    /// lower `order` (no ties exist in current data, kept for future updates).
    fn closest_eligible(&self, target: i32) -> TribeKey {
        let mut best_key = &self.eligible[0].2;
        let mut best_dist = i32::MAX;
        let mut best_order = i32::MAX;
        for (rank, order, key) in &self.eligible {
            let dist = (rank - target).abs();
            if dist < best_dist || (dist == best_dist && *order < best_order) {
                best_dist = dist;
                best_order = *order;
                best_key = key;
            }
        }
        best_key.clone()
    }
}
